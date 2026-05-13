# Cleanup duplicate helpers in key/peer_id generators

## Goal

提取 `key.rs` 和 `peer_id.rs` 之间重复的辅助函数和类型到 `generator/mod.rs`（或新建 `generator/common.rs`），减少 copy-paste 代码。不改动 `KeyGenerator` / `PeerIdGenerator` 的 enum 结构和行为。

## Background

grilling session 发现 candidate #2（两份 RefreshPolicy 副本）。本任务是第一步轻量整理，为后续泛型抽取做铺垫。

## Requirements

### R1 — 提取共用辅助函数

以下函数在 `key.rs` 和 `peer_id.rs` 中完全相同，提取到 `generator/common.rs`（pub(super)）：

- `lock_state<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T>`
- `default_shared_state<T: Default>() -> Arc<Mutex<T>>`
- `compile_rand_regex(pattern: &str) -> Result<RandRegex, ClientError>`
- `string_from_ascii_regex_bytes(bytes: Vec<u8>) -> Result<String, ClientError>`

### R2 — 提取 `AccessAware<T>` 泛型辅助类型

`AccessAwareKey` 和 `AccessAwarePeerId` 结构完全同构（字段名不同但语义相同）。提取为：

```rust
// generator/common.rs
#[derive(Debug, Clone)]
pub(super) struct AccessAwareEntry<T> {
    value: T,
    last_access: Instant,
    #[cfg(test)]
    force_stale: bool,
}
```

带 `new()` / `get()` / `should_evict()` / `mark_stale_for_test()` 方法。`key.rs` 和 `peer_id.rs` 改为 `type AccessAwareKey = AccessAwareEntry<String>` 或直接使用。

### R3 — 提取 `TimedState` 泛型辅助类型

`TimedKeyState` 和 `TimedPeerIdState` 同构：

```rust
#[derive(Debug, Clone, Default)]
pub(super) struct TimedState {
    pub value: Option<String>,
    pub last_generation: Option<Instant>,
}
```

### R4 — 零行为变更

- 所有 166+ 现有测试保持绿色
- `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets -- -D warnings` 通过
- `KeyGenerator` / `PeerIdGenerator` 的 public API 不变
- serde 序列化/反序列化行为不变

### R5 — 保留 peer_id 特有的 `TorrentPersistentPeerIdState`

`peer_id.rs` 的 `TORRENT_PERSISTENT` variant 有额外的 `get_counter` sweep 策略，这个 state 结构保留在 `peer_id.rs` 内部（组合 `AccessAwareEntry<String>` + counter）。

## Acceptance Criteria

- [ ] 新建 `rust/crates/joal-core/src/client/generator/common.rs`，包含 R1-R3 的类型和函数
- [ ] `key.rs` 和 `peer_id.rs` 中删除重复定义，改为 `use super::common::*`
- [ ] `TORRENT_PERSISTENT_TTL` 常量移到 `common.rs`（两边值相同，都是 2h）
- [ ] `generator/mod.rs` 加 `mod common;`（不 pub re-export，仅 pub(super)）
- [ ] `cargo test --workspace --no-fail-fast` 全绿
- [ ] `cargo fmt --all -- --check` + `cargo clippy --workspace --all-targets -- -D warnings` 通过

## Out of Scope

- 抽取 `KeyGenerator` / `PeerIdGenerator` 为泛型 `RefreshPolicy<Cfg>` — 留给后续 grill + 独立任务
- 改动 enum variant 结构
- 改动 public API
- 改动 serde 行为
