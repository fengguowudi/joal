# Extract generic RefreshPolicy from KeyGenerator and PeerIdGenerator

## Goal

把 `KeyGenerator` 和 `PeerIdGenerator` 两个几乎同构的 `refreshOn` enum 抽成泛型 `RefreshPolicy<C: GenerateValue>`，消除 enum variant 级别的重复代码（~400 行），保持 `.client` JSON 文件的 serde 兼容性。

## Background

Grill session（2026-05-13）决策汇总：

1. 用 `GenerateValue` trait 封装 algorithm + post-processing 差异
2. 泛型 enum 包含所有 variant 的超集（6 个），PeerId 侧 serde 自然拒绝不存在的 variant
3. `TORRENT_PERSISTENT` 统一为每次 `get()` 都 sweep（去掉 PeerId 的 counter 优化）
4. `GenerateValue` trait 包含 `generate()` + `validate()`，封装 algorithm 调用 + post-processing
5. 用 `#[serde(flatten)]` 保持 JSON 兼容（已验证可行）
6. `KeyGenerator` / `PeerIdGenerator` 变成 type alias
7. `get()` 签名保持 `(&self, info_hash: &InfoHash, event: RequestEvent)`
8. 通过 `RefreshPolicy::config() -> &C` accessor 暴露 config 字段
9. `RefreshPolicy<C>` 放新文件 `generator/refresh_policy.rs`
10. `GenerateValue` supertraits: `Clone + Debug + PartialEq + Eq + Serialize + Deserialize`

## Requirements

### R1 — `GenerateValue` trait

```rust
// generator/refresh_policy.rs
pub trait GenerateValue: Clone + Debug + PartialEq + Eq + Serialize + for<'de> Deserialize<'de> {
    fn generate(&self) -> Result<String, ClientError>;
    fn validate(&self) -> Result<(), ClientError>;
}
```

### R2 — `RefreshPolicy<C: GenerateValue>` enum

```rust
#[allow(non_camel_case_types)]
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "refreshOn")]
pub enum RefreshPolicy<C: GenerateValue> {
    NEVER {
        #[serde(flatten)]
        config: C,
        #[serde(skip, default = "default_shared_state::<TimedState>")]
        state: Arc<Mutex<TimedState>>,
    },
    ALWAYS {
        #[serde(flatten)]
        config: C,
    },
    TIMED {
        #[serde(rename = "refreshEvery")]
        refresh_every: i32,
        #[serde(flatten)]
        config: C,
        #[serde(skip, default = "default_shared_state::<TimedState>")]
        state: Arc<Mutex<TimedState>>,
    },
    TIMED_OR_AFTER_STARTED_ANNOUNCE {
        #[serde(rename = "refreshEvery")]
        refresh_every: i32,
        #[serde(flatten)]
        config: C,
        #[serde(skip, default = "default_shared_state::<TimedState>")]
        state: Arc<Mutex<TimedState>>,
    },
    TORRENT_VOLATILE {
        #[serde(flatten)]
        config: C,
        #[serde(skip, default = "default_shared_state::<HashMap<InfoHash, String>>")]
        state: Arc<Mutex<HashMap<InfoHash, String>>>,
    },
    TORRENT_PERSISTENT {
        #[serde(flatten)]
        config: C,
        #[serde(skip, default = "default_shared_state::<HashMap<InfoHash, AccessAwareEntry>>")]
        state: Arc<Mutex<HashMap<InfoHash, AccessAwareEntry>>>,
    },
}
```

手动实现 `Clone`（clone 时重置 state）和 `PartialEq`（忽略 state）。

### R3 — 统一 `get()` 实现

`RefreshPolicy<C>::get(&self, info_hash: &InfoHash, event: RequestEvent) -> Result<String, ClientError>` 一份代码，内部调 `config.generate()`。

`TORRENT_PERSISTENT` 统一为每次 `get()` 都 sweep（去掉 PeerId 原有的 counter）。

### R4 — `config()` accessor

```rust
impl<C: GenerateValue> RefreshPolicy<C> {
    pub fn config(&self) -> &C { ... }
    pub fn validate(&self) -> Result<(), ClientError> { ... }
}
```

### R5 — `KeyConfig` struct + `GenerateValue` impl

```rust
// key.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyConfig {
    pub algorithm: KeyAlgorithmDef,
    #[serde(rename = "keyCase")]
    pub key_case: Casing,
}

impl GenerateValue for KeyConfig { ... }

pub type KeyGenerator = RefreshPolicy<KeyConfig>;
```

### R6 — `PeerIdConfig` struct + `GenerateValue` impl

```rust
// peer_id.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerIdConfig {
    pub algorithm: PeerIdAlgorithmDef,
    #[serde(rename = "shouldUrlEncode")]
    pub should_url_encode: bool,
}

impl GenerateValue for PeerIdConfig { ... }

pub type PeerIdGenerator = RefreshPolicy<PeerIdConfig>;
```

### R7 — 消费侧适配

- `BitTorrentClient` 的 `.should_url_encode()` 改为 `.peer_id_generator.config().should_url_encode`
- `BitTorrentClient` 的 `.key_case()` 如有外部调用改为 `.key_generator.config().key_case`
- `BitTorrentClientConfig` 的 `validate()` 改为调 `.peer_id_generator.validate()` / `.key_generator.validate()`

### R8 — Serde 兼容性

现有 `.client` JSON 文件零改动即可反序列化。用 `#[serde(flatten)]` 平铺 config 字段。

### R9 — 零行为变更

- 所有现有测试保持绿色
- `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace --no-fail-fast` 全绿
- announce 请求字节级不变

## Acceptance Criteria

- [ ] 新建 `generator/refresh_policy.rs`，包含 `GenerateValue` trait + `RefreshPolicy<C>` enum + `get()` + `config()` + `validate()` + 手动 `Clone` / `PartialEq` / `Eq`
- [ ] `key.rs` 缩减为 algorithm 定义 + `KeyConfig` struct + `GenerateValue` impl + `pub type KeyGenerator`
- [ ] `peer_id.rs` 缩减为 algorithm 定义 + `PeerIdConfig` struct + `GenerateValue` impl + `pub type PeerIdGenerator`
- [ ] `TorrentPersistentPeerIdState` 删除，统一用 `HashMap<InfoHash, AccessAwareEntry>`
- [ ] `bit_torrent_client.rs` 适配 `.config()` accessor
- [ ] 现有 serde 测试（`serde_dispatch_matches_java_type_tag`、`generator_shell_parses_*`、`refresh_shell_parses_*`）全绿
- [ ] 现有行为测试（`never_reuses_*`、`timed_reuses_*`、`torrent_volatile_*`、`torrent_persistent_*`）全绿
- [ ] `cargo fmt + clippy + test` 全绿
- [ ] net 代码行数减少（目标 -200 行以上）

## Implementation Plan

1. 新建 `generator/refresh_policy.rs`：定义 `GenerateValue` trait + `RefreshPolicy<C>` enum（从 `key.rs` 的 `KeyGenerator` 搬运 variant 结构，替换具体类型为 `C`）
2. 实现 `RefreshPolicy<C>` 的 `get()` / `config()` / `validate()` + 手动 `Clone` / `PartialEq` / `Eq`
3. 在 `key.rs` 中定义 `KeyConfig` + impl `GenerateValue`，删除 `KeyGenerator` enum，加 `pub type KeyGenerator = RefreshPolicy<KeyConfig>`
4. 在 `peer_id.rs` 中定义 `PeerIdConfig` + impl `GenerateValue`，删除 `PeerIdGenerator` enum + `TorrentPersistentPeerIdState`，加 `pub type PeerIdGenerator = RefreshPolicy<PeerIdConfig>`
5. 更新 `generator/mod.rs` re-exports
6. 适配 `bit_torrent_client.rs` 消费侧
7. 跑 `cargo fmt + clippy + test`

## Out of Scope

- 改动 algorithm 定义（`KeyAlgorithmDef` / `PeerIdAlgorithmDef`）
- 改动 `.client` JSON 格式
- 改动 announce 行为
- 引入 egui / 其他 crate
