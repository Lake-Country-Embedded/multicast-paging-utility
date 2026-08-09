# multicast-paging-utility Python bindings

Monitor multicast paging groups from Python and pytest, with structured
per-page results: packet counts, loss, jitter, and audio metrics.

## Install

```bash
pip install -e tools/multicast-paging-utility/python
```

Registers a `pytest11` plugin, so the fixtures below need no conftest wiring.

## The arming barrier

This is the reason the package exists.

`MonitorHandle.wait_armed()` returns once **every multicast group has actually
been joined**, driven by the `armed` event that `test --json` emits after its
IGMP join loop. Before that event existed, a caller could only sleep and hope —
and a page triggered too early lost its opening packets, or was missed entirely,
so the test either flaked or quietly measured nothing.

```python
with client.monitor(zones, out_dir) as monitor:
    place_the_call()          # groups are already joined
monitor.summary.assert_delivered(zones[0])
```

`armed` proves the *local* join was issued. It does not prove an upstream
switch's IGMP snooping table has converged, so on a real switched network pass
`settle=0.5` — a named, tunable delay rather than a magic sleep. On a local
bridge, `0` is right.

## Quick start

```python
from multicast_utility import MulticastClient, PassCriteria, Zone

client = MulticastClient(interface="192.168.10.5")
zones = [Zone("224.1.1.1", 5000, zone_id=1), Zone("224.1.1.5", 5000, zone_id=2)]

with client.monitor(zones, out_dir, settle=0.5) as monitor:
    trigger_the_page()
    monitor.wait_page_ended(timeout=20)

summary = monitor.summary
summary.assert_delivered(zones[0], PassCriteria(min_packets=100, max_loss_percent=1.0))
summary.assert_silent(zones[1])        # the leak assertion
```

## pytest fixtures

| Fixture | Scope | What it gives you |
|---|---|---|
| `multicast_client` | session | A client bound to the detected or configured interface |
| `multicast_monitor` | function | `factory(zones) ->` context manager, armed on entry |
| `multicast_output_dir` | function | A **fresh** directory per monitor |
| `multicast_criteria` | function | Default pass thresholds |
| `multicast_interface` | session | `--multicast-interface`, else the `br0` address |
| `multicast_settle` | session | `--multicast-settle`, default `0` |

```python
def test_page_reaches_zone_one(multicast_monitor, zones, voip_client, sip_uri):
    with multicast_monitor(zones) as monitor:
        voip_client.call("ext6010", sip_uri("zone1"), play=TONE, hangup_after=6)
    monitor.summary.assert_delivered(zones[0])
    for other in zones[1:]:
        monitor.summary.assert_silent(other)
```

Each monitor gets its own output directory. That is not tidiness: results are
read from `<dir>/summary.json`, so two monitors sharing a directory means the
second overwrites the first — or a stale summary from a previous run is read as
the current result.

## Patterns, not ranges

`build_pattern()` always emits an explicit comma-separated list. Ranges are
seductive and wrong here: a range covering `.1` and `.5` also joins `.2`–`.4`,
so pages on zones nobody asked about get counted as results.

The comma syntax is new — `parse_range` previously rejected it, which meant any
non-contiguous zone set produced a pattern the tool could not parse. The process
exited immediately and the old wrapper logged a misleading "Monitor exited
early", so tests that should have failed instead skipped their assertions.

## Things the old wrapper got wrong, fixed here

- **Loss and jitter are real.** `monitor`'s `page_ended` event omits both, so
  consumers could only ever report zero. `test --json`'s carries them.
- **Every page is checked.** Loss and jitter used to be read from `pages[0]`
  while packets were summed across all pages, so a clean first page masked a
  lossy second one.
- **A missing summary raises.** A killed monitor is a failure, not an empty
  result to silently reconstruct from partial metrics.
- **Output is drained.** A reader thread consumes stdout continuously; the old
  wrapper left `--verbose` output undrained and could deadlock once the 64 KiB
  pipe buffer filled.
- **Zone identity survives.** `Zone.zone_id` is carried through, instead of
  being discarded and re-derived from `address:port`.

## Selftest

```bash
multicast-utility-selftest [--interface IP]
```

Seven checks including a real transmit/receive round trip and the untargeted-group
negative control. Run it before blaming a fixture — it proves the binary works
and IGMP joins succeed on this host.

## Tests

```bash
pytest tools/multicast-paging-utility/python/tests -q    # units, no binary needed
```
