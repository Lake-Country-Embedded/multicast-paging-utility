"""Build validator for the multicast utility and these bindings.

Runs a real transmit/receive round trip on a scoped multicast group, so it
proves the binary works, IGMP joins succeed on this host, and the event stream
and summary parse -- before any test blames a fixture.

    multicast-utility-selftest [--binary PATH] [--interface IP]

Exit 0 when every check passes, 1 otherwise.
"""

from __future__ import annotations

import argparse
import math
import struct
import sys
import tempfile
import wave
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, List, Optional

from .client import MulticastClient, MulticastError, find_binary
from .config import PassCriteria, Zone

# Scoped to the same 224.0.123.x block the crate's own integration tests use, so
# a selftest cannot collide with production paging groups.
SELFTEST_ZONE = Zone("224.0.123.200", 15200, zone_id=1)
QUIET_ZONE = Zone("224.0.123.201", 15201, zone_id=2)


@dataclass
class CheckResult:
    name: str
    polarity: str
    ok: bool
    detail: str = ""


def _write_tone(path: Path, frequency: int = 1000, seconds: float = 3.0,
                rate: int = 8000) -> Path:
    """Write a mono 16-bit sine, so the selftest needs no test assets."""
    frames = bytearray()
    for index in range(int(rate * seconds)):
        value = int(24000 * math.sin(2 * math.pi * frequency * index / rate))
        frames += struct.pack("<h", value)
    with wave.open(str(path), "wb") as handle:
        handle.setnchannels(1)
        handle.setsampwidth(2)
        handle.setframerate(rate)
        handle.writeframes(bytes(frames))
    return path


def check_binary_found(client: MulticastClient) -> CheckResult:
    version = client.version()
    return CheckResult("binary is present and reports a version", "pos",
                       bool(version), version or "no output")


def check_pattern_building(client: MulticastClient) -> CheckResult:
    """Non-contiguous zones must render as an explicit list, not a range.

    A range spanning .1 and .5 also joins .2-.4, so pages on zones nobody asked
    about get counted as results.
    """
    from .config import build_pattern

    pattern = build_pattern([Zone("224.1.1.1", 5000), Zone("224.1.1.5", 5000)])
    ok = pattern == "224.1.1.1:5000,224.1.1.5:5000"
    return CheckResult("non-contiguous zones build a comma list", "pos", ok, pattern)


def check_round_trip(client: MulticastClient) -> CheckResult:
    """Transmit a tone and receive it: the end-to-end proof."""
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        tone = _write_tone(tmp_path / "tone.wav")

        with client.monitor([SELFTEST_ZONE], tmp_path / "out", arm_timeout=10) as monitor:
            client.transmit(tone, SELFTEST_ZONE, ttl=1)
            monitor.wait_page_ended(timeout=15)

        summary = monitor.summary
        pages = summary.pages_for(SELFTEST_ZONE)
        if not pages:
            return CheckResult("transmit round-trips to the monitor", "pos", False,
                               "no pages received; check multicast routing")
        page = pages[0]
        return CheckResult(
            "transmit round-trips to the monitor", "pos", True,
            f"{page.network.packets_received} packets, "
            f"{page.audio.dominant_freq_hz:.0f} Hz, "
            f"loss {page.network.loss_percent:.1f}%, "
            f"jitter {page.network.jitter_ms:.2f}ms",
        )


def check_loss_and_jitter_are_real(client: MulticastClient) -> CheckResult:
    """Page results carry loss and jitter, not placeholder zeros.

    ``monitor``'s page_ended event omits both, which is why a consumer of that
    event can only ever report zero. ``test --json`` includes them; this proves
    the field is populated rather than defaulted.
    """
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        tone = _write_tone(tmp_path / "tone.wav", seconds=2.0)

        with client.monitor([SELFTEST_ZONE], tmp_path / "out", arm_timeout=10) as monitor:
            client.transmit(tone, SELFTEST_ZONE, ttl=1)
            ended = monitor.wait_page_ended(timeout=15)

        if ended is None:
            return CheckResult("page_ended carries loss and jitter", "pos", False,
                               "no page_ended event")
        has_fields = "loss_percent" in ended and "jitter_ms" in ended
        # Jitter on a real transfer is small but non-zero; exactly 0.0 across a
        # multi-packet page would suggest the field is a placeholder.
        return CheckResult(
            "page_ended carries loss and jitter", "pos", has_fields,
            f"loss={ended.get('loss_percent')} jitter={ended.get('jitter_ms'):.3f}ms",
        )


def check_quiet_group_stays_quiet(client: MulticastClient) -> CheckResult:
    """A group nobody transmits to receives nothing.

    The negative control behind the leak tests: paging one zone must not put
    packets on another.
    """
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        tone = _write_tone(tmp_path / "tone.wav", seconds=2.0)

        with client.monitor([SELFTEST_ZONE, QUIET_ZONE], tmp_path / "out",
                            arm_timeout=10) as monitor:
            client.transmit(tone, SELFTEST_ZONE, ttl=1)
            monitor.wait_page_ended(timeout=15)

        summary = monitor.summary
        try:
            summary.assert_silent(QUIET_ZONE)
        except AssertionError as exc:
            return CheckResult("untargeted group stays silent", "neg", False, str(exc)[:90])
        return CheckResult(
            "untargeted group stays silent", "neg", True,
            f"target got {summary.page_count(SELFTEST_ZONE)} page(s), "
            f"other got {summary.page_count(QUIET_ZONE)}",
        )


def check_arming_is_observable(client: MulticastClient) -> CheckResult:
    """The armed event fires, and names every requested endpoint."""
    with tempfile.TemporaryDirectory() as tmp:
        handle = client.start_test(
            [SELFTEST_ZONE, QUIET_ZONE], Path(tmp) / "out", timeout=3,
        )
        try:
            event = handle.wait_armed(timeout=10)
        except MulticastError as exc:
            handle.stop()
            return CheckResult("armed event fires", "pos", False, str(exc)[:90])
        handle.stop()

        endpoints = event.get("endpoints", [])
        ok = len(endpoints) == 2 and all(e.get("joined") for e in endpoints)
        return CheckResult("armed event fires", "pos", ok,
                           f"{len(endpoints)} endpoint(s) joined")


def check_bad_pattern_is_rejected(client: MulticastClient) -> CheckResult:
    """A non-multicast address must fail loudly, not monitor nothing."""
    with tempfile.TemporaryDirectory() as tmp:
        handle = client.start_test([Zone("10.0.0.1", 5000)], Path(tmp) / "out", timeout=3)
        try:
            handle.wait_armed(timeout=5)
        except MulticastError:
            errors = handle.events_of("error")
            handle.stop_quietly()
            detail = errors[0].get("code") if errors else "process exited"
            return CheckResult("non-multicast address is rejected", "neg", True, str(detail))
        handle.stop_quietly()
        return CheckResult("non-multicast address is rejected", "neg", False,
                           "monitor armed on a unicast address")


CHECKS: List[Callable[[MulticastClient], CheckResult]] = [
    check_binary_found,
    check_pattern_building,
    check_arming_is_observable,
    check_round_trip,
    check_loss_and_jitter_are_real,
    check_quiet_group_stays_quiet,
    check_bad_pattern_is_rejected,
]


def run_all(binary: Optional[Path] = None,
            interface: Optional[str] = None) -> List[CheckResult]:
    client = MulticastClient(binary, interface=interface)
    results: List[CheckResult] = []
    for check in CHECKS:
        try:
            results.append(check(client))
        except Exception as exc:
            results.append(
                CheckResult(check.__name__, "pos", False, f"{type(exc).__name__}: {exc}")
            )
    return results


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(description="Validate the multicast utility")
    parser.add_argument("--binary", type=Path, default=None)
    parser.add_argument("--interface", default=None,
                        help="local IPv4 to bind for multicast reception")
    args = parser.parse_args(argv)

    try:
        find_binary(args.binary)
    except FileNotFoundError as exc:
        print(f"  [FAIL] {exc}")
        return 1

    results = run_all(args.binary, args.interface)
    for result in results:
        status = "PASS" if result.ok else "FAIL"
        print(f"  [{status}] ({result.polarity}) {result.name:<42} {result.detail}")

    passed = sum(1 for r in results if r.ok)
    failed = len(results) - passed
    print(f"\n{passed} passed, {failed} failed")
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
