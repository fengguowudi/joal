# Extract SeedManager seam and EngineSnapshot

## Goal

把 `joal-core` 原本散落的"引擎生命周期 + 事件 + live-state"收束到**一个 deep module** `SeedManager`，并新增 `EngineSnapshot` 作为 per-frame 可消费的 live-state projection。顺手清理一对无 adapter 的孤儿 seam（`SpeedChangedListener` trait + `EngineEvent::SeedingSpeedsHasChanged`），让 `joal-app/src/main.rs` 从 ~449 行瘦身到 ~150 行，为 MVP-2 egui 前端提供唯一的 engine handle。

## Background

来自 improve-codebase-architecture grilling session（2026-05-12）发现的架构摩擦：

- **Finding #1 — SeedManager seam 缺席**：Java `SeedManager.init()/startSeeding()` 的有序 wiring（config → .client → bandwidth → watcher → orchestrator）在 Rust 只存在于 `joal-app/src/main.rs::boot`（100+ 行手写过程代码）+ `.trellis/spec/backend/directory-structure.md` 的散文里。`joal-core/src/seed_manager.rs` 仍是 5 行 doc-only stub。MVP-2 egui 的 `eframe::run_native` 要么重跑 boot，要么拿一个"engine handle"——今天这个 handle 就是一个 4-field `AppRuntime` 外加若干隐式 invariant。
- **Finding #3 — `main.rs` 把 composition / event 投影 / 轮询状态投影 捆成一团**：三件事生命周期和 interface 都不同，全部硬编码到 `tracing::info!`。egui 想要同样的投射但输出到 `ViewModel`。
- **Finding #6 — `EngineEvent` 只有 transition，GUI 每帧要的 live state 没 interface**：`report_status` 的 25 行 join（`orchestrator.seeding_announcer_facades()` × `bandwidth.get_seed_stat_for_torrent(ih)` × `speed_map()`）是 workspace 里唯一 join 点，没有 `EngineSnapshot` / `TorrentStatus` 类型。MVP-2 的 `TorrentListPanel` 出现那一刻就会跨两个 seam，join 逻辑漏到每个 widget。
- **Finding #4 顺手清理 — 孤儿 trait + 孤儿 event**：`BandwidthDispatcher` 有 `Option<Arc<dyn SpeedChangedListener>>` + `set_speed_listener()` 但生产代码零调用；`EngineEvent::SeedingSpeedsHasChanged` 有定义、有 match arm，但 workspace 零构造位点。两个相邻 seam 都没有 adapter。

Grilling 决策：**选 C（composition + watch<EngineSnapshot> 推送）+ 选 α（event-driven merger）+ 选 i（snapshot 管 state，events 管 transition）+ 独立子任务**。

## Requirements

### R1 — `SeedManager` 作为 joal-core 唯一 public composition root

```rust
// joal-core/src/seed_manager.rs
pub struct SeedManager { /* private */ }

impl SeedManager {
    pub async fn start(joal_conf: &Path) -> Result<Self, JoalError>;
    pub async fn stop(self) -> Result<(), JoalError>;
    pub fn events(&self) -> broadcast::Receiver<EngineEvent>;
    pub fn snapshot(&self) -> watch::Receiver<EngineSnapshot>;
    pub fn active_client_filename(&self) -> &str;
}
```

- **不 pub** `ClientOrchestrator` / `BandwidthDispatcher` / `TorrentFileProvider` / `reqwest::Client`。内部持有。
- `start()` 封装 `joal-app::boot` 全部 wiring（config → .client → bandwidth → watcher → orchestrator）并 spawn merger task。
- `stop()` 封装 reverse-order shutdown：`orchestrator.stop().await` → `torrent_provider.stop().await` → merger task abort → dispatcher 通过 `Drop` 回收 tokio task。
- `events()` / `snapshot()` 支持多订阅者（`broadcast::Receiver` / `watch::Receiver` 都可 clone）。

### R2 — 新增 `EngineSnapshot` + `TorrentStatus`（live-state projection）

```rust
// joal-core/src/snapshot.rs  (NEW module)
#[derive(Clone, Debug)]
pub struct EngineSnapshot {
    pub active_client_filename: String,
    pub global_upload_speed_bps: u64,
    pub torrents: Vec<TorrentStatus>,
}

#[derive(Clone, Debug)]
pub struct TorrentStatus {
    pub info_hash: InfoHash,
    pub name: String,
    pub total_size: u64,

    // 来自 BandwidthDispatcher::get_seed_stat_for_torrent
    pub uploaded_bytes: u64,
    pub current_speed_bps: u64,

    // 来自 Announcer (facade)
    pub last_known_interval: Option<u32>,
    pub last_known_seeders: Option<u32>,
    pub last_known_leechers: Option<u32>,
    pub consecutive_fails: u32,
    pub last_announced_at: Option<Instant>,
}
```

- `EngineSnapshot` 是 value type，`Clone` 便宜；`watch` 通道负责持有最近一帧。
- 同一 snapshot 是**coherent**的：一次 rebuild 读两端合并后原子发布。

### R3 — event-driven merger task（选 α）

- `SeedManager::start` 内部 spawn `merger` task，它持有：
  - `events_rx: broadcast::Receiver<EngineEvent>`（自订阅）
  - `poke_rx: mpsc::Receiver<MergerPoke>`（内部通道，不 pub）
- 触发源：
  1. `EngineEvent::TorrentFileAdded` / `TorrentFileDeleted` / `GlobalSeedStarted` / `GlobalSeedStopped` / `ConfigLoaded` / `TooManyAnnouncesFailedInARow` 等现有 transition 事件
  2. `BandwidthDispatcher::Inner::recompute_speeds` 完成后发 `MergerPoke::SpeedRecomputed`（替换 `SpeedChangedListener` 的 invocation 位置）
  3. `Announcer::announce` 成功/失败后发 `MergerPoke::AnnouncerUpdated(InfoHash)`（替换当前 facade 的轮询来源）
- 每个触发：调 `rebuild_snapshot()` → `watch_tx.send_replace(...)`。
- merger task 收到 shutdown signal（`stop()` 关闭 poke_tx + broadcast）后退出。

### R4 — 清理孤儿 seam（Finding #4）

**删除**：
- `joal-core/src/bandwidth/dispatcher.rs` 里 `SpeedChangedListener` trait + `speed_listener` 字段 + `set_speed_listener()` 方法 + 相应 invocation
- `joal-core/src/events.rs` 里 `EngineEvent::SeedingSpeedsHasChanged { speeds }` variant
- `joal-app/src/main.rs::log_event` 对应 match arm
- 相关 test 里的 `Counter` listener（替换为直接断言 snapshot 的形式）

**替换为**：`BandwidthDispatcher` 持有一个 `Option<mpsc::Sender<MergerPoke>>`（internal wire-up from `SeedManager::start`），`recompute_speeds` 收尾处 `try_send(MergerPoke::SpeedRecomputed)`。

### R5 — `joal-app/src/main.rs` 瘦身

**删除**：`boot()` / `AppRuntime` / `load_active_client` / `build_bandwidth_dispatcher` / `spawn_status_printer` / `report_status` / `log_announcer_status`

**保留并改写**：
```rust
#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let args = Args::parse();
    let seed_manager = SeedManager::start(&args.joal_conf).await?;
    let events_task = spawn_event_logger(seed_manager.events());
    let snapshot_task = spawn_snapshot_logger(seed_manager.snapshot());
    tokio::signal::ctrl_c().await.ok();
    seed_manager.stop().await?;
    events_task.abort(); snapshot_task.abort();
    Ok(())
}
```
`spawn_snapshot_logger` 每收到 `changed().await` 就 info! 一行 snapshot 摘要，保持现有 CLI 周期性输出的可观察行为。行数目标 ≤ 180 行。

### R6 — 字节级兼容 + 零行为变更（non-goal 反面）

- announcer 发给 tracker 的 HTTP 请求、query、headers、URL 拼接**一个字节都不能变**。
- config.json / clients/*.client / torrents/*.torrent 的读写路径、archive 规则**保持不变**。
- 本次是纯内部重构，不动 protocol / 持久化层任何契约。

## Acceptance Criteria

- [ ] `joal-core/src/seed_manager.rs` 不再是 5 行 stub；`start/stop/events/snapshot/active_client_filename` 五个方法按 R1 定义实现
- [ ] `joal-core/src/snapshot.rs` 新增，`EngineSnapshot` + `TorrentStatus` 按 R2 定义
- [ ] `ClientOrchestrator` / `BandwidthDispatcher` / `TorrentFileProvider` 不再在 `joal-core` 之外可见（检查：`joal-app` 只 import `SeedManager` + `EngineEvent` + `EngineSnapshot` + `TorrentStatus` + 必要的 error type）
- [ ] `SpeedChangedListener` trait / `set_speed_listener` / `speed_listener` 字段 / `EngineEvent::SeedingSpeedsHasChanged` 全部删除
- [ ] `joal-app/src/main.rs` ≤ 180 行，CLI 观察行为保持（启动参数 / 加载的 client / torrent 列表 / 周期 announce 摘要）
- [ ] 新增集成测试 `tests/seed_manager_snapshot.rs`：
  - [ ] `SeedManager::start` 后 `events().subscribe()` 能收到 `ConfigLoaded` + `TorrentFileAdded`
  - [ ] wiremock tracker 返回 `interval: 30` 后，对应 `TorrentStatus.last_known_interval == Some(30)`、`last_announced_at.is_some()`
  - [ ] `stop()` 后 `snapshot().changed().await` 变 `Err`（sender dropped）
- [ ] 原有 `tests/orchestrator_end_to_end.rs` / `tests/announcer_http.rs` / 166 个既有测试全绿（行为不变）
- [ ] `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace --no-fail-fast` 全绿
- [ ] `.trellis/spec/backend/database-guidelines.md` 追加 "Scenario: SeedManager composition seam"（start/stop/events/snapshot 契约 + shutdown 反序 + watch publisher ordering）
- [ ] `.trellis/spec/backend/directory-structure.md` 更新现有 "Scenario: Rust CLI boot sequence"，指向 `SeedManager::start`，标注 joal-app 只剩 `tracing` sink

## Implementation Plan

1. 新建 `joal-core/src/snapshot.rs` + 导出到 `lib.rs`；先占位 `EngineSnapshot/TorrentStatus` 类型。
2. 在 `bandwidth/dispatcher.rs` 中替换 `SpeedChangedListener` → `Option<mpsc::Sender<MergerPoke>>`；删 trait + setter + 相关 test。
3. 在 `ttorrent_client/client.rs` 的 `Announcer` 成功/失败回路里追加 `poke_tx.try_send(AnnouncerUpdated(...))`（如需，通过 response_handlers 链增加一个 handler 或在 `AnnouncerExecutor` 上加）。
4. 实现 `SeedManager::start/stop/events/snapshot`，内部 spawn merger task，把 `joal-app::boot` 的 wiring 整段搬进来。
5. 移除 `EngineEvent::SeedingSpeedsHasChanged`，同步清理 `main.rs::log_event` arm。
6. 改写 `joal-app/src/main.rs`：删 `boot/AppRuntime/report_status` 等，改为 `SeedManager::start` + snapshot/events 两个 logger task。
7. 写 `tests/seed_manager_snapshot.rs`（基于 wiremock）。
8. 跑 `cargo fmt/clippy/test`，确保 167+ 测试全绿。
9. 更新 spec（database-guidelines + directory-structure）。
10. Phase 3.4 commit（单个 refactor commit，不夹带工具链）。

## Out of Scope

- candidate #2（generator refresh-policy 两份副本抽共享 module）—— 独立子任务后续处理。
- candidate #5（`ttorrent_client/` vs `announcer/` 分层改名）—— 留给 MVP-3 Java-removal 阶段自然改名。
- candidate #7（`AnnouncerResolver` / `ClientNotificationSink` 两个 one-adapter trait）—— 低优，暂挂。
- 引入 egui / eframe —— 本子任务只做 joal-core seam 重构 + joal-app CLI 瘦身，egui 是 MVP-2 的下一个子任务。
- 任何 protocol / 持久化层行为变更。

## Technical Notes

- `watch` 通道选型：`tokio::sync::watch`，因为消费者（CLI logger / 未来 egui ViewModel）**总是只关心最新一帧**，历史值可丢。区别于 `broadcast`（transition 语义，不丢）。
- `MergerPoke` 通道用 `mpsc::channel(128)`，`try_send`；队满时丢弃 poke（无意义重复），不影响正确性因为下一次 poke 会重新 merge。
- Shutdown ordering 的 invariant：merger task 必须在 orchestrator/dispatcher 之前退出（否则会消费一个已关闭的 `poke_rx`），用 `JoinHandle + tokio::select!` 或显式 cancel token 实现。
- `EngineSnapshot` 大小估算：10 个 torrent 场景下 ~200 字节×10 = 2KB，`Clone` + `watch.send_replace` 可接受。
