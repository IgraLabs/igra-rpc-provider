## tx_perf — Transaction Submission Performance CLI

### Overview
`tx_perf` is a lightweight Rust CLI that parses existing provider logs (stdin, docker logs pipe, or file) and periodically prints performance summaries of transaction submission.
- Processed definition: a transaction is considered processed when an RPC success line is observed.
- Works with piped streams and regular files (with optional follow).
- Prints both sliding-window and cumulative (since start) summaries.

### Build
```bash
cargo build --release --bin tx_perf
```

### Quick start
- Pipe docker logs:
```bash
docker compose --profile frontend-w5 logs -f --tail 10 | ./target/release/tx_perf --summary-interval 60 --window 60 --format md
docker logs -f -n 10 rpc-provider-1 | ./target/release/tx_perf --summary-interval 60 --window 300
```
- From a file, with follow, markdown output:
```bash
tx_perf --input provider.log --follow --summary-interval 30 --window 300 --format md
```

### CLI options
- `--input <PATH>`: read from file instead of stdin
- `--follow`: tail file (like tail -f); only with `--input`
- `--summary-interval <SECONDS>`: how often to print summaries (default: 60)
- `--window <SECONDS>`: sliding-window length for “last N seconds” stats (default: 300)
- `--format <text|md>`: output format (default: text)
- `--top-slowest <N>`: show N slowest RPC-success samples (default: 5)
- `--show-dropped`: print count of dropped/unparsable lines to stderr each interval

### What the tool parses
The parser is tolerant to extra fields and ordering. It recognizes:
- RPC success and latency:
```
RPC RESPONSE [id={ID}, hash={HASH}]: Transaction processed successfully, time={DURATION}, payload_size={BYTES} bytes
```
- Mining success (provider or wallet logs):
```
... event_type="mining_success", mining_duration_ms={MILLISECONDS}, ...
... event_type="mining_success", mining_duration_seconds={SECONDS}, ...
Mining completed: {NONCES} nonces in {DURATION}, ...
```
- Queue timing and capacity:
```
TX [id={ID}, hash={HASH}]: Transaction queued successfully, queue_time={DURATION}, available_capacity={CAP}
```
- Processor success/failure:
```
TX_PROCESSOR [id={ID}, hash={HASH}]: Transaction processed successfully, time={DURATION}, ...
TX_PROCESSOR [id={ID}, hash={HASH}]: Transaction failed: {ERR}, time={DURATION}, ...
```
- Wallet send/failed:
```
WALLET_CALL [hash={HASH}]: Transaction accepted by wallet, payload_size={BYTES}, send_time={DURATION}
WALLET_CALL [hash={HASH}]: Send failed: {ERR}, time={DURATION}
```
Durations supported: `ns`, `us`/`µs`, `ms`, `s` (e.g., `850ms`, `1.23s`).
Capacity field: prefers `available_capacity` but also accepts `capacity`.

### Metrics
- RPC success latency quantiles: p50, p90, p95, p99, plus min/avg/max (µs reported as ms rounded)
- Mining time latency quantiles: p50, p90, p95, p99, plus min/avg/max (if observed)
- TPS: 1 / p50(RPC success latency). Example: p50=120ms → TPS≈8.33
- Errors: counts of processor/wallet error lines; top error messages by frequency
- Capacity: min and average capacity observed in the window (if present)
- Outliers: top N slowest RPC-success samples (id/hash + latency)
- Windows:
  - Sliding window over the last `--window` seconds
  - Cumulative metrics since start of input
- Empty intervals are skipped (no output when there were zero RPC successes in the interval)

### Output examples
Text:
```
Window=300s  success=400  errors=20
RPC latency (ms): p50=120  p90=180  p95=240  p99=420  min=75  avg=132  max=880
TPS (1/p50): 8.33
Mining time (ms): p50=95  p90=150  p95=210  p99=360  min=40  avg=110  max=520
Capacity: min=980  avg=992
Top errors: UTXO exhausted (12), Validation failed (5)
Slowest[5]: id=abc hash=0x123.. 880ms; id=def hash=0x456.. 760ms

Overall  success=12840  errors=730
RPC latency (ms): p50=110  p90=170  p95=230  p99=390  min=60  avg=124  max=910
TPS (1/p50): 9.09
Mining time (ms): p50=100  p90=160  p95=220  p99=380  min=55  avg=118  max=700
```

Markdown:
```
### Tx Performance (last 300s)
- Success: 400 · Errors: 20 · TPS (1/p50): 8.33
- RPC latency (ms): p50 120 · p90 180 · p95 240 · p99 420 · min 75 · avg 132 · max 880
- Mining time (ms): p50 95 · p90 150 · p95 210 · p99 360 · min 40 · avg 110 · max 520
- Capacity: min 980 · avg 992
- Top errors: UTXO exhausted (12), Validation failed (5)
- Slowest[5]: [abc 0x123…] 880ms; [def 0x456…] 760ms

### Cumulative (since start)
- Success: 12,840 · Errors: 730 · TPS (1/p50): 9.09
- Mining time (ms): p50 100 · p90 160 · p95 220 · p99 380 · min 55 · avg 118 · max 700
```

### Signals and EOF
- Prints a final summary on EOF, Ctrl‑C (SIGINT), and (on Unix) SIGTERM.

### Notes and limitations
- Stream processing with low memory overhead; designed for long-running pipes
- Unparsable lines are ignored (optionally counted with `--show-dropped`)
- Error grouping is by exact string; may be adjusted if structured codes are added
- Default histogram bound ~100ms with adaptive growth if higher latencies are seen
- No JSON output (only text or markdown), no journald/syslog ingestion


