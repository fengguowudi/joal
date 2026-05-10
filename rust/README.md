# joal-rust (MVP)

Rust rewrite of JOAL. See the root `README.md` for the original Java project
and `.trellis/tasks/05-10-rewrite-joal-in-rust-with-egui-frontend/prd.md` for
the rewrite plan.

## Layout

```
rust/
├── Cargo.toml             # workspace root (resolver = 2, edition 2024)
└── crates/
    ├── joal-core/         # headless domain logic (no UI deps)
    ├── joal-app/          # `joal-desktop` binary (CLI + future egui UI)
    └── joal-testing/      # shared fixtures + Java golden samples
```

## Build

```
cd rust
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test  --workspace
cargo run   -p joal-app -- --joal-conf /path/to/joal-conf
```

Minimum toolchain: rustc 1.92 / edition 2024 (pinned via `rust-toolchain.toml`).

## Status

**Step S1 complete**: workspace scaffolding only. Modules are empty stubs; see
the PRD for the S2–S11 rollout order.
