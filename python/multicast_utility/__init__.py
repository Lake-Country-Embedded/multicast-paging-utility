"""Python bindings for the multicast paging utility.

Monitors multicast paging groups and reports structured, per-page results:
packet counts, loss, jitter, and audio metrics.

The important part is the arming barrier. ``MonitorHandle.wait_armed()`` returns
once every group has actually been joined, so a caller knows when it is safe to
trigger a page. Without it the only options were a blind sleep or scraping prose,
and a page triggered too early silently lost its opening packets -- or all of
them.

    from multicast_utility import MulticastClient, Zone

    client = MulticastClient(interface="192.168.10.5")
    zones = [Zone("224.1.1.1", 5000, zone_id=1)]

    with client.monitor(zones, out_dir) as monitor:
        place_the_call()

    monitor.summary.assert_delivered(zones[0])
"""

from .client import MonitorHandle, MulticastClient, MulticastError, find_binary
from .config import PassCriteria, Zone, build_pattern
from .results import (
    AudioSummary,
    EndpointTotal,
    MetricSnapshot,
    NetworkSummary,
    PageSummary,
    TestSummary,
)

__version__ = "0.1.0"

__all__ = [
    "AudioSummary",
    "EndpointTotal",
    "MetricSnapshot",
    "MonitorHandle",
    "MulticastClient",
    "MulticastError",
    "NetworkSummary",
    "PageSummary",
    "PassCriteria",
    "TestSummary",
    "Zone",
    "build_pattern",
    "find_binary",
]
