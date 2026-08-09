"""Endpoint and pass-criteria models.

The package owns address-pattern construction. That is deliberate: the previous
consumer built patterns by hand and, for any non-contiguous set of zones, emitted
a comma-joined string the tool could not parse -- so the process exited
immediately and the wrapper logged the misleading "Monitor exited early". Tests
that should have failed instead skipped their assertions.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Iterable, List, Optional, Sequence, Union

__all__ = ["PassCriteria", "Zone", "build_pattern"]


@dataclass(frozen=True)
class Zone:
    """One multicast endpoint, optionally tagged with the zone id that owns it.

    ``zone_id`` is carried through to results so a caller can map a page back to
    the zone it configured, instead of re-deriving it from ``address:port``.
    """

    address: str
    port: int = 5000
    zone_id: Optional[int] = None
    name: Optional[str] = None

    @property
    def endpoint(self) -> str:
        return f"{self.address}:{self.port}"

    def __str__(self) -> str:
        return self.endpoint


ZoneLike = Union[Zone, str, tuple]


def _coerce(value: ZoneLike) -> Zone:
    if isinstance(value, Zone):
        return value
    if isinstance(value, tuple):
        address, port = value
        return Zone(address=str(address), port=int(port))
    text = str(value)
    if ":" in text:
        address, _, port = text.rpartition(":")
        return Zone(address=address, port=int(port))
    return Zone(address=text)


def build_pattern(zones: Iterable[ZoneLike]) -> str:
    """Render zones as an address pattern the tool accepts.

    Always emits a comma-separated list of explicit endpoints rather than trying
    to collapse them into a range. Ranges are seductive but wrong here: a range
    covering ``.1`` and ``.5`` also joins ``.2``, ``.3`` and ``.4``, so pages on
    zones the caller did not ask about get counted as results.

    Duplicates are dropped, order preserved.
    """
    seen: List[str] = []
    for zone in zones:
        endpoint = _coerce(zone).endpoint
        if endpoint not in seen:
            seen.append(endpoint)
    if not seen:
        raise ValueError("no zones given; nothing to monitor")
    return ",".join(seen)


@dataclass(frozen=True)
class PassCriteria:
    """Thresholds a page must meet to count as delivered.

    Defaults match what the previous wrapper used, so migrated tests keep their
    meaning.
    """

    min_packets: int = 100
    max_loss_percent: float = 1.0
    max_jitter_ms: float = 50.0
    min_duration_sec: float = 3.0

    def evaluate(self, page) -> List[str]:
        """Return the reasons ``page`` fails, empty when it passes."""
        failures: List[str] = []
        if page.network.packets_received < self.min_packets:
            failures.append(
                f"{page.network.packets_received} packets < {self.min_packets} required"
            )
        if page.network.loss_percent > self.max_loss_percent:
            failures.append(
                f"loss {page.network.loss_percent:.2f}% > {self.max_loss_percent}%"
            )
        if page.network.jitter_ms > self.max_jitter_ms:
            failures.append(
                f"jitter {page.network.jitter_ms:.2f}ms > {self.max_jitter_ms}ms"
            )
        if page.duration_secs < self.min_duration_sec:
            failures.append(
                f"duration {page.duration_secs:.2f}s < {self.min_duration_sec}s"
            )
        return failures
