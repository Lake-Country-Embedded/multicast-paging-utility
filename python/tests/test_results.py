"""Unit tests for the multicast bindings. No binary, no network."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from multicast_utility import PassCriteria, TestSummary, Zone, build_pattern
from multicast_utility.results import MetricSnapshot, PageSummary

ZONE1 = Zone("224.1.1.1", 5000, zone_id=1)
ZONE2 = Zone("224.1.1.5", 5000, zone_id=2)


def _page(endpoint="224.1.1.1:5000", number=1, packets=150, loss=0.0,
          jitter=0.5, duration=4.0, freq=1000.0):
    return {
        "page_number": number,
        "endpoint": endpoint,
        "start_time": "2026-01-01T00:00:00Z",
        "end_time": "2026-01-01T00:00:04Z",
        "duration_secs": duration,
        "recording_file": f"page_{number:04d}.wav",
        "network": {
            "packets_received": packets, "bytes_received": packets * 160,
            "packets_lost": 0, "loss_percent": loss, "jitter_ms": jitter,
        },
        "audio": {
            "peak_rms_db": -8.0, "avg_rms_db": -12.0, "max_peak_db": -5.0,
            "dominant_freq_hz": freq, "total_glitches": 0, "total_clipped": 0,
            "clipping_percent": 0.0, "avg_zero_crossing_rate": 0.1,
        },
    }


def _write_summary(directory: Path, pages, errors=None):
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "summary.json").write_text(json.dumps({
        "test_metadata": {
            "start_time": "2026-01-01T00:00:00Z", "end_time": "2026-01-01T00:00:10Z",
            "duration_secs": 10.0, "pattern": "224.1.1.1:5000",
            "endpoints_monitored": 1, "metrics_interval_ms": 500, "timeout_secs": 10,
        },
        "pages": pages,
        "endpoint_totals": {},
        "errors": errors or [],
    }))
    return directory


# --- pattern building -------------------------------------------------------

def test_non_contiguous_zones_build_a_comma_list():
    """A range would also join the groups in between, and count their pages."""
    assert build_pattern([ZONE1, ZONE2]) == "224.1.1.1:5000,224.1.1.5:5000"


def test_pattern_deduplicates_and_keeps_order():
    assert build_pattern([ZONE2, ZONE1, ZONE2]) == "224.1.1.5:5000,224.1.1.1:5000"


def test_pattern_accepts_strings_and_tuples():
    assert build_pattern(["224.1.1.1:5000"]) == "224.1.1.1:5000"
    assert build_pattern([("224.1.1.2", 5001)]) == "224.1.1.2:5001"
    assert build_pattern(["224.1.1.3"]) == "224.1.1.3:5000"


def test_empty_pattern_is_an_error():
    with pytest.raises(ValueError, match="no zones"):
        build_pattern([])


# --- summary queries --------------------------------------------------------

def test_summary_loads_and_queries_by_zone(tmp_path):
    summary = TestSummary.load(_write_summary(tmp_path / "out", [_page()]))
    assert summary.page_count(ZONE1) == 1
    assert summary.page_count(ZONE2) == 0
    assert summary.saw_traffic(ZONE1) is True
    assert summary.total_packets(ZONE1) == 150
    assert summary.endpoints_with_traffic == ["224.1.1.1:5000"]


def test_missing_summary_raises_rather_than_reporting_empty(tmp_path):
    """A killed monitor is a failure, not a zero result."""
    (tmp_path / "out").mkdir()
    with pytest.raises(FileNotFoundError, match="did not shut down cleanly"):
        TestSummary.load(tmp_path / "out")


def test_avg_rms_none_is_preserved(tmp_path):
    """Omitted means 'no valid samples', which is not the same as very quiet."""
    page = _page()
    del page["audio"]["avg_rms_db"]
    summary = TestSummary.load(_write_summary(tmp_path / "out", [page]))
    assert summary.pages[0].audio.avg_rms_db is None


# --- pass criteria ----------------------------------------------------------

def test_delivery_passes_on_a_clean_page(tmp_path):
    summary = TestSummary.load(_write_summary(tmp_path / "out", [_page()]))
    assert summary.evaluate(ZONE1) == []
    summary.assert_delivered(ZONE1)


def test_every_page_is_checked_not_just_the_first(tmp_path):
    """A clean first page must not mask a lossy second one."""
    summary = TestSummary.load(_write_summary(tmp_path / "out", [
        _page(number=1, loss=0.0),
        _page(number=2, loss=12.0),
    ]))
    failures = summary.evaluate(ZONE1)
    assert len(failures) == 1
    assert "page 2" in failures[0]
    assert "12.00%" in failures[0]

    with pytest.raises(AssertionError, match="page 2"):
        summary.assert_delivered(ZONE1)


def test_criteria_flag_each_dimension(tmp_path):
    summary = TestSummary.load(_write_summary(tmp_path / "out", [
        _page(packets=10, loss=5.0, jitter=99.0, duration=0.5),
    ]))
    failures = " ".join(summary.evaluate(ZONE1))
    assert "packets" in failures
    assert "loss" in failures
    assert "jitter" in failures
    assert "duration" in failures


def test_no_pages_is_a_failure_not_a_pass(tmp_path):
    summary = TestSummary.load(_write_summary(tmp_path / "out", []))
    assert summary.evaluate(ZONE1) == ["no pages received on 224.1.1.1:5000"]


def test_custom_criteria_are_honored(tmp_path):
    summary = TestSummary.load(_write_summary(tmp_path / "out", [_page(packets=10)]))
    lenient = PassCriteria(min_packets=5, min_duration_sec=1.0)
    assert summary.evaluate(ZONE1, lenient) == []


# --- leak assertion ---------------------------------------------------------

def test_assert_silent_passes_on_an_untouched_zone(tmp_path):
    summary = TestSummary.load(_write_summary(tmp_path / "out", [_page()]))
    summary.assert_silent(ZONE2)


def test_assert_silent_fails_when_traffic_leaked(tmp_path):
    summary = TestSummary.load(_write_summary(
        tmp_path / "out", [_page(endpoint="224.1.1.5:5000")]
    ))
    with pytest.raises(AssertionError, match="expected no multicast on 224.1.1.5:5000"):
        summary.assert_silent(ZONE2)


# --- metrics ----------------------------------------------------------------

def test_peak_rms_uses_only_active_page_samples(tmp_path):
    out = _write_summary(tmp_path / "out", [_page()])
    (out / "metrics.jsonl").write_text("\n".join(json.dumps(m) for m in [
        {"timestamp": "t", "endpoint": "224.1.1.1:5000", "page_active": False,
         "network": {"packets": 0, "bytes": 0, "loss_percent": 0.0, "jitter_ms": 0.0},
         "audio": {"rms_db": -96.0, "peak_db": -96.0, "dominant_freq_hz": 0.0,
                   "glitches": 0, "clipped": 0}},
        {"timestamp": "t", "endpoint": "224.1.1.1:5000", "page_active": True,
         "network": {"packets": 50, "bytes": 8000, "loss_percent": 0.0, "jitter_ms": 0.4},
         "audio": {"rms_db": -20.0, "peak_db": -10.0, "dominant_freq_hz": 1000.0,
                   "glitches": 0, "clipped": 0}},
        {"timestamp": "t", "endpoint": "224.1.1.1:5000", "page_active": True,
         "network": {"packets": 100, "bytes": 16000, "loss_percent": 0.0, "jitter_ms": 0.5},
         "audio": {"rms_db": -14.0, "peak_db": -8.0, "dominant_freq_hz": 1000.0,
                   "glitches": 0, "clipped": 0}},
    ]))
    summary = TestSummary.load(out)
    assert summary.peak_rms_db(ZONE1) == -14.0
    assert summary.peak_rms_db(ZONE2) == -96.0


def test_metrics_skip_malformed_lines(tmp_path):
    out = _write_summary(tmp_path / "out", [_page()])
    (out / "metrics.jsonl").write_text(
        '{"broken\n'
        '{"timestamp":"t","endpoint":"224.1.1.1:5000","page_active":true,'
        '"network":{"packets":1,"bytes":1,"loss_percent":0,"jitter_ms":0},'
        '"audio":{"rms_db":-30,"peak_db":-20,"dominant_freq_hz":1000,'
        '"glitches":0,"clipped":0}}\n'
    )
    assert len(list(TestSummary.load(out).metrics())) == 1


def test_page_address_and_port_split():
    page = PageSummary.from_json(_page(endpoint="239.1.2.3:5060"))
    assert page.address == "239.1.2.3"
    assert page.port == 5060


def test_metric_snapshot_uses_the_jsonl_field_names():
    """The snapshot names differ from the summary's; both must map correctly."""
    snapshot = MetricSnapshot.from_json({
        "timestamp": "t", "endpoint": "e", "page_active": True,
        "network": {"packets": 7, "bytes": 8, "loss_percent": 1.5, "jitter_ms": 2.5},
        "audio": {"rms_db": -30.0, "peak_db": -20.0, "dominant_freq_hz": 440.0,
                  "glitches": 3, "clipped": 4},
    })
    assert snapshot.packets == 7        # summary calls this packets_received
    assert snapshot.glitches == 3       # summary calls this total_glitches
    assert snapshot.loss_percent == 1.5
