# Default `simultaneousSeed = 200` Benchmark

## Command

```bash
cargo test -p joal-app benchmark_default_200_profile --release -- --ignored --nocapture
```

## What The Harness Measures

The benchmark lives in `crates/joal-app/src/ui/benchmark.rs` and builds a
representative 200-torrent workspace with:

- 200 `TorrentStatus` rows
- the torrent table search/sort model enabled
- the resizable workspace layout active
- 180 log entries
- 300 upload-speed history points
- the config panel visible

It reports three main cost buckets:

1. `approx_workspace_bytes`:
   a deep-size estimate of the in-memory UI workspace model
   (`EngineSnapshot` + log buffer + speed history), not OS-level RSS.
2. `snapshot_clone_avg_us`:
   average CPU time to clone the 200-torrent `EngineSnapshot`.
3. `ui_frame_avg_ms`:
   average CPU time to build one full egui workspace frame at `1600x900`.

## Recorded Result

Run date: `2026-05-16`

Environment summary:

- OS: Windows
- Logical parallelism: `12`
- Profile: `release`

Observed output:

```text
default-200 benchmark
  torrents: 200
  approx_workspace_bytes: 67555 (0.06 MiB)
  snapshot_clone_avg_us: 22.19
  ui_frame_avg_ms: 1.215
  avg_shape_count: 496.0
  available_parallelism: 12
```

## Interpretation

- The current 200-row workspace model is cheap to clone relative to a
  16.7 ms / 60 FPS frame budget.
- Full UI frame construction stayed around `1.2 ms` per frame in `release`
  mode on the measured host, which leaves substantial headroom for the rest
  of the desktop app.
- The memory number is intentionally a deterministic model-size estimate.
  If OS working-set tracking becomes necessary later, add a platform-backed
  RSS probe as a second metric rather than replacing this one.
