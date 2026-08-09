"""Pytest fixtures for multicast paging verification.

Registered as a ``pytest11`` entry point.

``multicast_monitor`` is the one most tests want: a context manager that starts
monitoring, waits for the groups to be joined, and loads results on exit.
"""

from __future__ import annotations

import logging
import re
import subprocess
from pathlib import Path
from typing import Iterator, List, Optional

import pytest

from .client import MulticastClient, find_binary
from .config import PassCriteria, Zone

logger = logging.getLogger(__name__)


def pytest_addoption(parser):
    group = parser.getgroup("multicast")
    group.addoption(
        "--multicast-interface",
        action="store",
        default=None,
        help="Local IPv4 address to bind for multicast reception, e.g. the br0 "
             "bridge address. Required on hosts with several interfaces, where "
             "the default picks the wrong one and nothing is ever received.",
    )
    group.addoption(
        "--multicast-settle",
        action="store",
        type=float,
        default=0.0,
        help="Extra delay after the groups are joined, for switch IGMP snooping "
             "to converge. 0 is right on a local bridge; a real switched network "
             "may want 0.5-1.0.",
    )


def _bridge_interface_ip() -> Optional[str]:
    """IPv4 address of the br0 bridge, if there is one.

    Deliberately does not consider VLAN sub-interfaces: on those rigs the
    interface must be named explicitly, because guessing picks the wrong one and
    the failure looks like "the DUT never transmitted".
    """
    try:
        result = subprocess.run(
            ["ip", "-4", "-o", "addr", "show", "br0"],
            capture_output=True, text=True, timeout=5,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return None
    if result.returncode != 0:
        return None
    match = re.search(r"inet (\d+\.\d+\.\d+\.\d+)", result.stdout)
    return match.group(1) if match else None


@pytest.fixture(scope="session")
def multicast_binary() -> Path:
    try:
        return find_binary()
    except FileNotFoundError as exc:
        pytest.skip(str(exc))


@pytest.fixture(scope="session")
def multicast_interface(request) -> Optional[str]:
    explicit = request.config.getoption("--multicast-interface")
    if explicit:
        return explicit
    detected = _bridge_interface_ip()
    if detected:
        logger.info("using detected bridge interface %s for multicast", detected)
    return detected


@pytest.fixture(scope="session")
def multicast_settle(request) -> float:
    return request.config.getoption("--multicast-settle")


@pytest.fixture(scope="session")
def multicast_client(multicast_binary, multicast_interface) -> MulticastClient:
    return MulticastClient(multicast_binary, interface=multicast_interface)


@pytest.fixture
def multicast_output_dir(tmp_path) -> "callable":
    """Factory for a fresh output directory per monitor.

    Every monitor needs its own: results are read from ``<dir>/summary.json``,
    so two monitors sharing a directory means the second overwrites the first --
    or worse, a stale summary from a previous run is read as the current result.
    """
    counter = {"n": 0}

    def factory(name: str = "multicast") -> Path:
        counter["n"] += 1
        path = tmp_path / f"{name}_{counter['n']}"
        path.mkdir(parents=True, exist_ok=True)
        return path

    return factory


@pytest.fixture
def multicast_monitor(multicast_client, multicast_output_dir, multicast_settle):
    """Monitor zones for the duration of a ``with`` block.

    ::

        with multicast_monitor(zones) as monitor:
            place_the_call()
        monitor.summary.assert_delivered(zones[0])

    The groups are joined before the block body runs, so a page triggered inside
    it cannot be missed.
    """
    def factory(zones, *, name: str = "multicast", settle: Optional[float] = None, **kwargs):
        return multicast_client.monitor(
            zones,
            multicast_output_dir(name),
            settle=multicast_settle if settle is None else settle,
            **kwargs,
        )

    return factory


@pytest.fixture
def multicast_criteria() -> PassCriteria:
    """Default delivery thresholds. Override per-module where a test needs different ones."""
    return PassCriteria()
