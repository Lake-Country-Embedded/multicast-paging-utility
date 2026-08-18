"""Typed views over ``summary.json``, ``metrics.jsonl``, and the event stream.

Mapped 1:1 from the Rust structs in ``src/cli/test.rs``. Note the deliberate
name divergence upstream between the per-interval snapshot and the per-page
summary (``network.packets`` vs ``network.packets_received``, ``audio.glitches``
vs ``audio.total_glitches``); both are represented faithfully rather than being
papered over, so a mismatch surfaces here rather than as a silent zero.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, Iterator, List, Mapping, Optional

__all__ = [
    "AudioSummary",
    "EndpointTotal",
    "MetricSnapshot",
    "NetworkSummary",
    "PageSummary",
    "TestSummary",
]


@dataclass(frozen=True)
class NetworkSummary:
    packets_received: int = 0
    bytes_received: int = 0
    packets_lost: int = 0
    loss_percent: float = 0.0
    jitter_ms: float = 0.0

    @classmethod
    def from_json(cls, raw: Mapping[str, Any]) -> "NetworkSummary":
        return cls(
            packets_received=int(raw.get("packets_received", 0)),
            bytes_received=int(raw.get("bytes_received", 0)),
            packets_lost=int(raw.get("packets_lost", 0)),
            loss_percent=float(raw.get("loss_percent", 0.0)),
            jitter_ms=float(raw.get("jitter_ms", 0.0)),
        )


def _opt_float(raw: Mapping[str, Any], key: str) -> Optional[float]:
    """Read a float that the analyzer may report as JSON null.

    A page carrying only digital silence has no measurable RMS, and the Rust
    analyzer emits ``"peak_rms_db": null`` rather than omitting the key. A plain
    ``raw.get(key, default)`` returns ``None`` in that case -- the default only
    applies to a *missing* key -- so ``float()`` then raises and the whole
    summary fails to parse. That turns "the stream was silent", which is a
    legitimate and interesting result, into an unparseable run.
    """
    value = raw.get(key)
    return None if value is None else float(value)


@dataclass(frozen=True)
class AudioSummary:
    # None when no valid samples were seen -- a page of pure silence, or one too
    # short to analyze. Meaningfully different from a number: do not collapse it
    # to -96, or "no audio at all" reads as "very quiet".
    peak_rms_db: Optional[float] = None
    avg_rms_db: Optional[float] = None
    max_peak_db: Optional[float] = None
    dominant_freq_hz: float = 0.0
    total_glitches: int = 0
    total_clipped: int = 0
    clipping_percent: float = 0.0
    avg_zero_crossing_rate: float = 0.0

    @property
    def silent(self) -> bool:
        """No measurable audio content, however many packets arrived."""
        return self.peak_rms_db is None

    @classmethod
    def from_json(cls, raw: Mapping[str, Any]) -> "AudioSummary":
        return cls(
            peak_rms_db=_opt_float(raw, "peak_rms_db"),
            avg_rms_db=_opt_float(raw, "avg_rms_db"),
            max_peak_db=_opt_float(raw, "max_peak_db"),
            dominant_freq_hz=float(raw.get("dominant_freq_hz") or 0.0),
            total_glitches=int(raw.get("total_glitches") or 0),
            total_clipped=int(raw.get("total_clipped") or 0),
            clipping_percent=float(raw.get("clipping_percent") or 0.0),
            avg_zero_crossing_rate=float(raw.get("avg_zero_crossing_rate") or 0.0),
        )


@dataclass(frozen=True)
class PageSummary:
    page_number: int
    endpoint: str
    duration_secs: float
    recording_file: str
    network: NetworkSummary
    audio: AudioSummary
    start_time: str = ""
    end_time: str = ""

    @property
    def address(self) -> str:
        return self.endpoint.rsplit(":", 1)[0]

    @property
    def port(self) -> int:
        return int(self.endpoint.rsplit(":", 1)[1])

    def recording_path(self, output_dir: Path) -> Path:
        return Path(output_dir) / self.recording_file

    @classmethod
    def from_json(cls, raw: Mapping[str, Any]) -> "PageSummary":
        return cls(
            page_number=int(raw.get("page_number", 0)),
            endpoint=str(raw.get("endpoint", "")),
            duration_secs=float(raw.get("duration_secs", 0.0)),
            recording_file=str(raw.get("recording_file", "")),
            network=NetworkSummary.from_json(raw.get("network", {})),
            audio=AudioSummary.from_json(raw.get("audio", {})),
            start_time=str(raw.get("start_time", "")),
            end_time=str(raw.get("end_time", "")),
        )


@dataclass(frozen=True)
class EndpointTotal:
    pages_detected: int = 0
    total_duration_secs: float = 0.0
    total_packets: int = 0
    total_bytes: int = 0

    @classmethod
    def from_json(cls, raw: Mapping[str, Any]) -> "EndpointTotal":
        return cls(
            pages_detected=int(raw.get("pages_detected", 0)),
            total_duration_secs=float(raw.get("total_duration_secs", 0.0)),
            total_packets=int(raw.get("total_packets", 0)),
            total_bytes=int(raw.get("total_bytes", 0)),
        )


@dataclass(frozen=True)
class MetricSnapshot:
    """One per-endpoint sample from ``metrics.jsonl``.

    Written every interval whether or not a page is active, so a caller can see
    what the endpoint was doing between pages as well as during them.
    """

    timestamp: str
    endpoint: str
    page_active: bool
    page_number: Optional[int]
    duration_secs: Optional[float]
    packets: int
    bytes: int
    loss_percent: float
    jitter_ms: float
    # None when the snapshot carried no measurable audio -- a window of pure
    # silence still produces packets, so "quiet" and "no traffic" are different
    # states and must stay distinguishable.
    rms_db: Optional[float]
    peak_db: Optional[float]
    dominant_freq_hz: float
    glitches: int
    clipped: int

    @property
    def silent(self) -> bool:
        """Packets arrived (or not) but carried no measurable audio."""
        return self.rms_db is None

    @classmethod
    def from_json(cls, raw: Mapping[str, Any]) -> "MetricSnapshot":
        network = raw.get("network", {})
        audio = raw.get("audio", {})
        return cls(
            timestamp=str(raw.get("timestamp", "")),
            endpoint=str(raw.get("endpoint", "")),
            page_active=bool(raw.get("page_active", False)),
            page_number=raw.get("page_number"),
            duration_secs=raw.get("duration_secs"),
            packets=int(network.get("packets", 0)),
            bytes=int(network.get("bytes", 0)),
            loss_percent=float(network.get("loss_percent", 0.0)),
            jitter_ms=float(network.get("jitter_ms", 0.0)),
            rms_db=_opt_float(audio, "rms_db"),
            peak_db=_opt_float(audio, "peak_db"),
            dominant_freq_hz=float(audio.get("dominant_freq_hz") or 0.0),
            glitches=int(audio.get("glitches") or 0),
            clipped=int(audio.get("clipped") or 0),
        )


@dataclass
class TestSummary:
    """Everything a completed run produced."""

    # Named for the `test` subcommand, not for pytest; without this pytest tries
    # to collect it as a test class and warns about the constructor.
    __test__ = False

    output_dir: Path
    pattern: str = ""
    duration_secs: float = 0.0
    endpoints_monitored: int = 0
    pages: List[PageSummary] = field(default_factory=list)
    endpoint_totals: Dict[str, EndpointTotal] = field(default_factory=dict)
    errors: List[str] = field(default_factory=list)

    # -- queries -----------------------------------------------------------

    def pages_for(self, endpoint) -> List[PageSummary]:
        """Pages seen on one endpoint. Accepts a ``Zone``, a string, or a tuple."""
        from .config import _coerce  # local import to avoid a cycle

        wanted = _coerce(endpoint).endpoint if not isinstance(endpoint, str) else endpoint
        if ":" not in wanted:
            return [p for p in self.pages if p.address == wanted]
        return [p for p in self.pages if p.endpoint == wanted]

    def page_count(self, endpoint) -> int:
        return len(self.pages_for(endpoint))

    def saw_traffic(self, endpoint) -> bool:
        return self.page_count(endpoint) > 0

    def total_packets(self, endpoint) -> int:
        return sum(p.network.packets_received for p in self.pages_for(endpoint))

    @property
    def endpoints_with_traffic(self) -> List[str]:
        return sorted({p.endpoint for p in self.pages})

    # -- assertions --------------------------------------------------------

    def evaluate(self, endpoint, criteria=None) -> List[str]:
        """Reasons ``endpoint`` fails ``criteria``, empty when it passes.

        Every page is checked, not just the first. The previous wrapper read
        loss and jitter from ``pages[0]`` alone while summing packets across all
        of them, so a clean first page masked a lossy second one.
        """
        from .config import PassCriteria

        criteria = criteria or PassCriteria()
        pages = self.pages_for(endpoint)
        if not pages:
            return [f"no pages received on {endpoint}"]

        failures: List[str] = []
        for page in pages:
            for reason in criteria.evaluate(page):
                failures.append(f"page {page.page_number}: {reason}")
        return failures

    def assert_delivered(self, endpoint, criteria=None) -> List[PageSummary]:
        """Raise unless ``endpoint`` received traffic meeting ``criteria``."""
        failures = self.evaluate(endpoint, criteria)
        if failures:
            raise AssertionError(
                f"multicast delivery failed on {endpoint}:\n  " + "\n  ".join(failures)
            )
        return self.pages_for(endpoint)

    def assert_silent(self, endpoint) -> None:
        """Raise if any traffic arrived on ``endpoint``.

        The leak assertion: paging one zone must not put packets on another.
        """
        pages = self.pages_for(endpoint)
        if pages:
            detail = ", ".join(
                f"page {p.page_number} ({p.network.packets_received} packets)" for p in pages
            )
            raise AssertionError(f"expected no multicast on {endpoint}, got {detail}")

    # -- loading -----------------------------------------------------------

    def metrics(self) -> Iterator[MetricSnapshot]:
        """Stream ``metrics.jsonl``, skipping malformed lines."""
        path = self.output_dir / "metrics.jsonl"
        if not path.is_file():
            return
        with open(path) as handle:
            for line in handle:
                line = line.strip()
                if not line:
                    continue
                try:
                    yield MetricSnapshot.from_json(json.loads(line))
                except (ValueError, KeyError):
                    continue

    def peak_rms_db(self, endpoint) -> float:
        """Loudest RMS observed on ``endpoint`` while a page was active.

        Used by the volume tests, which compare measured attenuation against
        ``20*log10(v2/v1)``.
        """
        from .config import _coerce

        wanted = _coerce(endpoint).endpoint if not isinstance(endpoint, str) else endpoint
        levels = [
            m.rms_db for m in self.metrics()
            if m.page_active and m.endpoint == wanted and m.rms_db is not None
        ]
        return max(levels) if levels else -96.0

    @classmethod
    def load(cls, output_dir) -> "TestSummary":
        """Read a completed run's ``summary.json``.

        Raises ``FileNotFoundError`` when the summary is absent, which means the
        tool was killed before it could write one -- a real failure, not an empty
        result. The previous wrapper silently reconstructed a partial answer from
        ``metrics.jsonl`` in that case, which hid crashes.
        """
        output_dir = Path(output_dir)
        path = output_dir / "summary.json"
        if not path.is_file():
            raise FileNotFoundError(
                f"{path} not written; the monitor did not shut down cleanly"
            )

        with open(path) as handle:
            raw = json.load(handle)

        metadata = raw.get("test_metadata", {})
        return cls(
            output_dir=output_dir,
            pattern=str(metadata.get("pattern", "")),
            duration_secs=float(metadata.get("duration_secs", 0.0)),
            endpoints_monitored=int(metadata.get("endpoints_monitored", 0)),
            pages=[PageSummary.from_json(p) for p in raw.get("pages", [])],
            endpoint_totals={
                key: EndpointTotal.from_json(value)
                for key, value in raw.get("endpoint_totals", {}).items()
            },
            errors=list(raw.get("errors", [])),
        )
