"""Drive the multicast-paging-utility binary from Python.

The shape that matters is :class:`MonitorHandle`: start monitoring, **wait until
the groups are actually joined**, trigger a page by some other means, then stop
and read structured results.

That middle step is the point of this package. Previously the only options were
a blind ``time.sleep(1)`` or scraping a prose line, so a page triggered too early
missed its opening packets -- or the whole page -- and the test either flaked or
quietly measured nothing.
"""

from __future__ import annotations

import json
import logging
import os
import queue
import shutil
import subprocess
import threading
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Dict, Iterable, Iterator, List, Optional, Union

from .config import PassCriteria, Zone, ZoneLike, build_pattern
from .results import TestSummary

logger = logging.getLogger(__name__)

__all__ = ["MonitorHandle", "MulticastClient", "MulticastError", "find_binary"]

PathLike = Union[str, Path]

_BINARY_NAME = "multicast-paging-utility"

# Search order mirrors voip-utility's: in-tree build first, then the system.
_CANDIDATES = (
    Path(__file__).resolve().parents[2] / "target" / "release" / _BINARY_NAME,
    Path(__file__).resolve().parents[2] / "target" / "debug" / _BINARY_NAME,
    Path("/usr/local/bin") / _BINARY_NAME,
    Path("/usr/bin") / _BINARY_NAME,
)


class MulticastError(RuntimeError):
    """The tool failed, or was asked for something it cannot do."""


def find_binary(binary_path: Optional[PathLike] = None) -> Path:
    if binary_path:
        path = Path(binary_path)
        if not path.is_file():
            raise FileNotFoundError(f"multicast utility not found at {path}")
        return path

    for candidate in _CANDIDATES:
        if candidate.is_file():
            return candidate

    found = shutil.which(_BINARY_NAME)
    if found:
        return Path(found)

    raise FileNotFoundError(
        f"{_BINARY_NAME} not found. Build it with "
        f"`cargo build --release` in tools/multicast-paging-utility."
    )


class MonitorHandle:
    """A running ``test`` process with an observable armed state.

    Events arrive on stdout as JSONL and are drained by a reader thread, so the
    pipe never fills -- the previous wrapper left ``--verbose`` output undrained
    and could deadlock mid-test once the 64 KiB buffer filled.
    """

    def __init__(self, proc: subprocess.Popen, output_dir: Path, argv: List[str]):
        self._proc = proc
        self._argv = argv
        self.output_dir = output_dir
        self.events: List[Dict[str, Any]] = []
        self._queue: "queue.Queue[Dict[str, Any]]" = queue.Queue()
        self._lock = threading.Lock()
        self._stderr: List[str] = []

        self._reader = threading.Thread(target=self._read_events, daemon=True)
        self._reader.start()
        self._stderr_pump = threading.Thread(target=self._read_stderr, daemon=True)
        self._stderr_pump.start()

    # -- plumbing ----------------------------------------------------------

    def _read_events(self) -> None:
        if self._proc.stdout is None:
            return
        for line in self._proc.stdout:
            line = line.strip()
            if not line:
                continue
            try:
                event = json.loads(line)
            except ValueError:
                logger.debug("non-JSON line from monitor: %s", line[:200])
                continue
            with self._lock:
                self.events.append(event)
            self._queue.put(event)

    def _read_stderr(self) -> None:
        if self._proc.stderr is None:
            return
        for line in self._proc.stderr:
            with self._lock:
                self._stderr.append(line)

    @property
    def stderr(self) -> str:
        with self._lock:
            return "".join(self._stderr)

    def running(self) -> bool:
        return self._proc.poll() is None

    def events_of(self, event_type: str) -> List[Dict[str, Any]]:
        with self._lock:
            return [e for e in self.events if e.get("event") == event_type]

    # -- synchronization ---------------------------------------------------

    def wait_for(self, event_type: str, timeout: float = 10.0) -> Optional[Dict[str, Any]]:
        """Block until an event of ``event_type`` arrives, or the deadline passes."""
        with self._lock:
            for event in self.events:
                if event.get("event") == event_type:
                    return event

        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return None
            try:
                event = self._queue.get(timeout=min(remaining, 0.25))
            except queue.Empty:
                if not self.running():
                    return None
                continue
            if event.get("event") == event_type:
                return event

    def wait_armed(self, timeout: float = 10.0, settle: float = 0.0) -> Dict[str, Any]:
        """Block until every multicast group has been joined.

        This is the barrier to take before triggering a page.

        ``settle`` adds an explicit extra delay afterwards. The ``armed`` event
        proves the *local* IGMP join was issued; it does not prove an upstream
        switch's snooping table has converged. On a real switched network give
        this a small value -- but as a named, tunable parameter rather than the
        scattered magic sleeps it replaces.
        """
        event = self.wait_for("armed", timeout=timeout)
        if event is None:
            raise MulticastError(
                f"monitor did not arm within {timeout}s "
                f"(running={self.running()}). stderr: {self.stderr[-400:]}"
            )
        if settle:
            time.sleep(settle)
        return event

    def wait_page_started(self, timeout: float = 30.0) -> Optional[Dict[str, Any]]:
        return self.wait_for("page_started", timeout=timeout)

    def wait_page_ended(self, timeout: float = 30.0) -> Optional[Dict[str, Any]]:
        return self.wait_for("page_ended", timeout=timeout)

    # -- teardown ----------------------------------------------------------

    def stop_quietly(self, timeout: float = 10.0) -> None:
        """Terminate without reading results.

        For runs that never got far enough to produce a summary -- a rejected
        address pattern, say -- where :meth:`stop` would raise
        ``FileNotFoundError`` and mask the real failure.
        """
        if self._proc.poll() is None:
            self._proc.terminate()
            try:
                self._proc.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                logger.warning("monitor ignored SIGTERM; killing")
                self._proc.kill()
                self._proc.wait(timeout=5)
        self._reader.join(timeout=2.0)
        self._stderr_pump.join(timeout=2.0)

    def stop(self, timeout: float = 10.0) -> TestSummary:
        """Stop monitoring and load the results.

        SIGTERM rather than SIGKILL: the tool handles it and writes a complete
        summary, so a caller does not have to wait out the full ``--timeout``.

        Raises ``FileNotFoundError`` when no summary was written, which means the
        run died before finishing -- a real failure worth surfacing, not an empty
        result to paper over.
        """
        self.stop_quietly(timeout=timeout)

        # The summary is written during shutdown; give it a moment to land.
        deadline = time.monotonic() + 3.0
        summary_path = self.output_dir / "summary.json"
        while not summary_path.is_file() and time.monotonic() < deadline:
            time.sleep(0.05)

        return TestSummary.load(self.output_dir)

    def __enter__(self) -> "MonitorHandle":
        return self

    def __exit__(self, *_exc) -> None:
        if self.running():
            self.stop()


class MulticastClient:
    """Start monitors and transmit test audio."""

    def __init__(
        self,
        binary_path: Optional[PathLike] = None,
        *,
        interface: Optional[str] = None,
        default_port: int = 5000,
    ):
        self.binary = find_binary(binary_path)
        self.interface = interface
        self.default_port = default_port

    # -- monitoring --------------------------------------------------------

    def start_test(
        self,
        zones: Iterable[ZoneLike],
        output_dir: PathLike,
        *,
        timeout: Optional[float] = None,
        metrics_interval_ms: int = 500,
        interface: Optional[str] = None,
        codec: Optional[str] = None,
    ) -> MonitorHandle:
        """Start monitoring ``zones``; returns before they are joined.

        Call :meth:`MonitorHandle.wait_armed` before triggering a page.

        ``timeout=None`` runs until stopped, which is usually what a test wants:
        the caller knows when the page is over, and guessing an upper bound just
        adds dead time to every run.
        """
        output_dir = Path(output_dir)
        output_dir.mkdir(parents=True, exist_ok=True)

        argv = [
            str(self.binary), "test",
            "--address", build_pattern(zones),
            "--port", str(self.default_port),
            "--output", str(output_dir),
            "--timeout", str(int(timeout) if timeout else 0),
            "--metrics-interval", str(metrics_interval_ms),
            "--json",
        ]
        chosen_interface = interface or self.interface
        if chosen_interface:
            argv += ["--interface", chosen_interface]
        if codec:
            argv += ["--codec", codec]

        logger.debug("starting monitor: %s", " ".join(argv))
        proc = subprocess.Popen(
            argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        return MonitorHandle(proc, output_dir, argv)

    @contextmanager
    def monitor(
        self,
        zones: Iterable[ZoneLike],
        output_dir: PathLike,
        *,
        arm_timeout: float = 10.0,
        settle: float = 0.0,
        **kwargs: Any,
    ) -> Iterator[MonitorHandle]:
        """Monitor for the duration of the ``with`` block, armed on entry.

        ::

            with client.monitor(zones, out_dir) as monitor:
                place_the_call()          # groups are already joined
            summary = monitor.summary     # populated on exit

        The handle carries ``.summary`` after the block, so the caller does not
        have to remember to call :meth:`MonitorHandle.stop`.
        """
        handle = self.start_test(zones, output_dir, **kwargs)
        try:
            handle.wait_armed(timeout=arm_timeout, settle=settle)
            yield handle
        finally:
            handle.summary = handle.stop()  # type: ignore[attr-defined]

    # -- transmitting ------------------------------------------------------

    def transmit(
        self,
        audio_file: PathLike,
        zone: ZoneLike,
        *,
        codec: str = "g711ulaw",
        ttl: int = 32,
        loop: bool = False,
        timeout: float = 60.0,
    ) -> None:
        """Send a WAV to a multicast group. Raises on failure.

        Used to stand in for a DUT when validating the rig itself.
        """
        from .config import _coerce

        target = _coerce(zone)
        argv = [
            str(self.binary), "transmit",
            "--file", str(Path(audio_file).resolve()),
            "--address", target.address,
            "--port", str(target.port),
            "--codec", codec,
            "--ttl", str(ttl),
        ]
        if loop:
            argv.append("--loop")

        result = subprocess.run(argv, capture_output=True, text=True, timeout=timeout)
        if result.returncode != 0:
            raise MulticastError(
                f"transmit to {target} failed (exit {result.returncode}): "
                f"{result.stderr.strip()[:300]}"
            )

    def version(self) -> str:
        result = subprocess.run(
            [str(self.binary), "--version"], capture_output=True, text=True, timeout=10
        )
        return result.stdout.strip()
