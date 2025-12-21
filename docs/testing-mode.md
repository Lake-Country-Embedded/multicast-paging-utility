# Automated Testing Mode

The `test` command provides a CI/CD-friendly mode for automated testing of multicast paging systems. It monitors multicast addresses, records pages, and outputs structured metrics for automated analysis.

## Quick Start

```bash
# Monitor a single address for 60 seconds
multicast-paging-utility test \
    --address 224.0.1.1 \
    --port 5004 \
    --output ./test-results \
    --timeout 60

# Monitor multiple addresses with range syntax
multicast-paging-utility test \
    --address "224.0.{1-5}.1:{5000-5004}" \
    --output ./test-results \
    --timeout 300
```

## Command Options

| Option | Short | Required | Default | Description |
|--------|-------|----------|---------|-------------|
| `--address` | `-a` | Yes | - | Multicast address pattern (supports ranges) |
| `--port` | `-p` | No | 5004 | Default UDP port |
| `--codec` | `-c` | No | auto | Force codec: g711ulaw, g711alaw, opus, l16 |
| `--output` | `-o` | Yes | - | Output directory for results |
| `--timeout` | `-t` | Yes | - | Test duration in seconds |
| `--metrics-interval` | - | No | 500 | Metrics sampling interval (ms) |

## Output Files

The test command creates a flat directory structure:

```
output-dir/
├── metrics.jsonl           # Timestamped metrics (JSON Lines)
├── summary.json            # Final test summary
├── page_0001_224_0_1_1_5004.wav
├── page_0002_224_0_1_1_5004.wav
└── ...
```

### metrics.jsonl

One JSON object per line, sampled at the configured interval:

```json
{"timestamp":"2024-01-15T10:30:00.500Z","endpoint":"224.0.1.1:5004","page_active":true,"page_number":1,"duration_secs":5.2,"network":{"packets":260,"bytes":41600,"loss_percent":0.0,"jitter_ms":1.2},"audio":{"rms_db":-18.5,"peak_db":-6.2,"dominant_freq_hz":1000.0,"glitches":0,"clipped":0}}
```

Fields:
- `timestamp` - ISO 8601 timestamp
- `endpoint` - Address:port being monitored
- `page_active` - Whether a page is currently being received
- `page_number` - Current page number (if active)
- `duration_secs` - Current page duration (if active)
- `network.packets` - Packets received so far
- `network.bytes` - Bytes received so far
- `network.loss_percent` - Packet loss percentage
- `network.jitter_ms` - Network jitter in milliseconds
- `audio.rms_db` - Current RMS level in dB
- `audio.peak_db` - Current peak level in dB
- `audio.dominant_freq_hz` - Dominant frequency detected
- `audio.glitches` - Total glitches detected
- `audio.clipped` - Total clipped samples

### summary.json

Complete test summary written at the end:

```json
{
  "test_metadata": {
    "start_time": "2024-01-15T10:30:00Z",
    "end_time": "2024-01-15T10:35:00Z",
    "duration_secs": 300.0,
    "pattern": "224.0.1.1:5004",
    "endpoints_monitored": 1,
    "metrics_interval_ms": 500,
    "timeout_secs": 300
  },
  "pages": [
    {
      "page_number": 1,
      "endpoint": "224.0.1.1:5004",
      "start_time": "2024-01-15T10:30:05Z",
      "end_time": "2024-01-15T10:30:35Z",
      "duration_secs": 30.0,
      "recording_file": "page_0001_224_0_1_1_5004.wav",
      "network": {
        "packets_received": 1500,
        "bytes_received": 240000,
        "packets_lost": 2,
        "loss_percent": 0.13,
        "jitter_ms": 1.2
      },
      "audio": {
        "peak_rms_db": -12.5,
        "avg_rms_db": -18.3,
        "max_peak_db": -3.2,
        "dominant_freq_hz": 1000.0,
        "total_glitches": 0,
        "total_clipped": 0,
        "clipping_percent": 0.0,
        "avg_zero_crossing_rate": 2500.0
      }
    }
  ],
  "endpoint_totals": {
    "224.0.1.1:5004": {
      "pages_detected": 1,
      "total_duration_secs": 30.0,
      "total_packets": 1500,
      "total_bytes": 240000
    }
  },
  "errors": []
}
```

## CI/CD Integration

### GitHub Actions

```yaml
name: Paging System Test

on: [push, pull_request]

jobs:
  test-paging:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Start paging system
        run: ./start-paging-system.sh &

      - name: Run paging test
        run: |
          multicast-paging-utility test \
            --address 224.0.1.1:5004 \
            --output ./test-results \
            --timeout 60

      - name: Check results
        run: |
          # Check that at least one page was detected
          PAGES=$(jq '.pages | length' ./test-results/summary.json)
          if [ "$PAGES" -eq 0 ]; then
            echo "No pages detected!"
            exit 1
          fi

          # Check for errors
          ERRORS=$(jq '.errors | length' ./test-results/summary.json)
          if [ "$ERRORS" -gt 0 ]; then
            echo "Errors detected:"
            jq '.errors' ./test-results/summary.json
            exit 1
          fi

          # Check packet loss threshold
          LOSS=$(jq '.pages[0].network.loss_percent' ./test-results/summary.json)
          if (( $(echo "$LOSS > 1.0" | bc -l) )); then
            echo "Packet loss too high: $LOSS%"
            exit 1
          fi

      - name: Upload test artifacts
        uses: actions/upload-artifact@v4
        if: always()
        with:
          name: paging-test-results
          path: ./test-results/
```

### GitLab CI

```yaml
paging-test:
  stage: test
  script:
    - ./start-paging-system.sh &
    - sleep 5
    - multicast-paging-utility test
        --address 224.0.1.1:5004
        --output ./test-results
        --timeout 60
    - |
      PAGES=$(jq '.pages | length' ./test-results/summary.json)
      if [ "$PAGES" -eq 0 ]; then
        echo "No pages detected!"
        exit 1
      fi
  artifacts:
    when: always
    paths:
      - test-results/
    expire_in: 1 week
```

### Jenkins Pipeline

```groovy
pipeline {
    agent any

    stages {
        stage('Test Paging') {
            steps {
                sh './start-paging-system.sh &'
                sh 'sleep 5'
                sh '''
                    multicast-paging-utility test \
                        --address 224.0.1.1:5004 \
                        --output ./test-results \
                        --timeout 60
                '''
            }
            post {
                always {
                    archiveArtifacts artifacts: 'test-results/**'
                }
            }
        }

        stage('Validate Results') {
            steps {
                script {
                    def summary = readJSON file: 'test-results/summary.json'

                    if (summary.pages.size() == 0) {
                        error 'No pages detected'
                    }

                    if (summary.errors.size() > 0) {
                        error "Errors: ${summary.errors}"
                    }

                    summary.pages.each { page ->
                        if (page.network.loss_percent > 1.0) {
                            error "High packet loss: ${page.network.loss_percent}%"
                        }
                        if (page.audio.total_glitches > 0) {
                            unstable "Glitches detected: ${page.audio.total_glitches}"
                        }
                    }
                }
            }
        }
    }
}
```

## Parsing Results with jq

Common queries for the summary.json file:

```bash
# Get number of pages detected
jq '.pages | length' summary.json

# Get total duration of all pages
jq '[.pages[].duration_secs] | add' summary.json

# Check if any errors occurred
jq '.errors | length' summary.json

# Get average packet loss across all pages
jq '[.pages[].network.loss_percent] | add / length' summary.json

# Find pages with glitches
jq '.pages[] | select(.audio.total_glitches > 0)' summary.json

# Get all recording filenames
jq -r '.pages[].recording_file' summary.json

# Check if specific endpoint received pages
jq '.endpoint_totals["224.0.1.1:5004"].pages_detected' summary.json
```

## Parsing metrics.jsonl

```bash
# Get all metrics for a specific endpoint
grep '"endpoint":"224.0.1.1:5004"' metrics.jsonl

# Find when pages started
grep '"page_active":true' metrics.jsonl | head -1

# Get peak RMS levels over time
jq -s '[.[].audio.rms_db] | max' metrics.jsonl

# Count metrics samples
wc -l metrics.jsonl

# Convert to CSV for analysis
jq -r '[.timestamp, .endpoint, .page_active, .audio.rms_db] | @csv' metrics.jsonl
```

## Python Analysis Example

```python
import json
from pathlib import Path

def analyze_test_results(output_dir: str):
    output_path = Path(output_dir)

    # Load summary
    with open(output_path / "summary.json") as f:
        summary = json.load(f)

    print(f"Test duration: {summary['test_metadata']['duration_secs']:.1f}s")
    print(f"Pages detected: {len(summary['pages'])}")
    print(f"Errors: {len(summary['errors'])}")

    # Analyze each page
    for page in summary['pages']:
        print(f"\nPage {page['page_number']} ({page['endpoint']}):")
        print(f"  Duration: {page['duration_secs']:.1f}s")
        print(f"  Packets: {page['network']['packets_received']}")
        print(f"  Loss: {page['network']['loss_percent']:.2f}%")
        print(f"  Glitches: {page['audio']['total_glitches']}")
        print(f"  Avg RMS: {page['audio']['avg_rms_db']:.1f} dB")

    # Load and analyze metrics
    metrics = []
    with open(output_path / "metrics.jsonl") as f:
        for line in f:
            metrics.append(json.loads(line))

    print(f"\nMetrics samples: {len(metrics)}")

    # Find glitch events
    glitch_samples = [m for m in metrics if m['audio']['glitches'] > 0]
    if glitch_samples:
        print(f"Samples with glitches: {len(glitch_samples)}")

if __name__ == "__main__":
    analyze_test_results("./test-results")
```

## Exit Codes

The test command always exits with code 0, regardless of test results. This allows external tools to parse the output files and make their own pass/fail decisions based on custom thresholds.

All errors are captured in the `summary.json` file in the `errors` array.

## Audio Quality Metrics

The following audio metrics help identify quality issues:

| Metric | Description | Typical Threshold |
|--------|-------------|-------------------|
| `rms_db` | RMS level (loudness) | > -50 dB (not silent) |
| `peak_db` | Peak amplitude | < -1 dB (not clipping) |
| `glitches` | Sudden amplitude jumps | 0 (no glitches) |
| `clipped` | Samples at max value | < 0.1% of total |
| `loss_percent` | Network packet loss | < 1% |
| `jitter_ms` | Network jitter | < 50ms |
