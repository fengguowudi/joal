# MVP2: egui Frontend — Torrent Dashboard with Real-Time Monitoring

## Goal

在已完成的 headless Rust 引擎之上，用 `eframe 0.34` 构建原生桌面 GUI。窗口消费 `SeedManager::snapshot_watch()` 提供的 `watch::Receiver<EngineSnapshot>` 实时渲染做种状态，替代 MVP-1 的 CLI 状态打印。

## Context

- `SeedManager` 是唯一的 composition root，提供：
  - `snapshot_watch()` → `watch::Receiver<EngineSnapshot>`（per-frame 状态投影）
  - `subscribe_events()` → `broadcast::Receiver<EngineEvent>`（transition 事件流）
  - `stop()` → 优雅关闭
- `EngineSnapshot` 包含：`active_client_filename`、`global_upload_speed_bps`、`Vec<TorrentStatus>`
- `TorrentStatus` 包含：`info_hash`、`name`、`total_size`、`uploaded_bytes`、`current_speed_bps`、`last_known_interval`、`last_known_seeders`、`last_known_leechers`、`consecutive_fails`、`last_announced_at`
- workspace 已声明 `eframe = "0.34"` / `egui` / `egui_extras` / `egui_plot` 依赖

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  joal-app (binary)                                  │
│                                                     │
│  main() ─┬─ parse Args (--joal-conf)               │
│           ├─ init tracing (with egui bridge)        │
│           ├─ spawn tokio runtime on background thread│
│           ├─ SeedManager::start() on that runtime   │
│           └─ eframe::run_native(JoalApp)            │
│                                                     │
│  JoalApp (eframe::App)                              │
│    ├─ snapshot_rx: watch::Receiver<EngineSnapshot>  │
│    ├─ events_rx: broadcast::Receiver<EngineEvent>   │
│    ├─ log_buffer: VecDeque<LogEntry> (ring, 500)    │
│    ├─ speed_history: VecDeque<(f64, f64)> (300pts)  │
│    └─ update() → poll snapshot + events → render    │
└─────────────────────────────────────────────────────┘
```

### tokio + eframe 集成模式

eframe 占据主线程的事件循环。tokio runtime 在独立 OS 线程上运行：

```rust
let rt = tokio::runtime::Runtime::new()?;
let seed_manager = rt.block_on(SeedManager::start(&joal_conf))?;
// pass snapshot_rx / events_rx into JoalApp
eframe::run_native(options, Box::new(|_cc| Ok(Box::new(app))));
// on exit:
rt.block_on(seed_manager.stop());
```

`update()` 每帧非阻塞 poll `snapshot_rx.has_changed()` 和 drain `events_rx.try_recv()`。当 snapshot 变化时调用 `ctx.request_repaint()` 保持 UI 响应。

## Requirements

### R1 主窗口 — Torrent 列表

- 表格列：Name | Info Hash (前 8 hex) | Upload Speed | Uploaded | Seeders | Leechers | Interval | Status
- Status 列：显示 `consecutive_fails > 0` 时为黄色警告，`> 3` 为红色
- 按 upload speed 降序默认排列
- 空状态：居中提示 "No torrents loaded — add .torrent files to your torrents/ folder"

### R2 全局状态栏

- 顶部横条：Active Client 名称 | Total Upload Speed (human-readable, e.g. "1.2 MB/s") | Torrent Count
- 底部状态栏：运行时长 (HH:MM:SS) | Engine status indicator (green dot = running)

### R3 实时速度曲线

- 使用 `egui_plot` 在主窗口下方或侧面板绘制 total upload speed 时间序列
- 保留最近 5 分钟数据点（每秒采样 → 300 点）
- Y 轴自适应，X 轴滚动

### R4 日志面板

- 底部可折叠面板，显示最近 500 条 `EngineEvent` 格式化日志
- 每条日志：时间戳 + 事件类型 + 关键字段
- 自动滚动到底部，可手动暂停滚动

### R5 启停控制

- 全局 Start / Stop 按钮，控制做种引擎的运行状态
- Stop 时发送所有 torrent 的 STOPPED announce，然后停止引擎
- Start 重新加载 config 并启动引擎
- 按钮状态反映当前引擎状态（Running → 显示 Stop 按钮，Stopped → 显示 Start 按钮）

### R6 配置编辑面板

- 侧面板或弹窗形式，可编辑以下字段：
  - `minUploadRate` (kB/s) — 数字输入
  - `maxUploadRate` (kB/s) — 数字输入
  - `simultaneousSeed` — 数字输入
  - `client` — 下拉选择，列出 `clients/` 目录下所有 `.client` 文件
  - `uploadRatioTarget` — 数字输入（-1 表示无限制）
- Save 按钮将配置写入 `config.json` 并重载引擎
- 显示当前生效的配置值

### R7 Torrent 文件管理

- 删除 torrent：torrent 列表每行右侧有删除按钮，点击后将 .torrent 文件移到 archive 目录
- 添加 torrent：通过文件对话框选择 .torrent 文件，复制到 `torrents/` 目录（watcher 自动感知）
- 删除确认对话框防止误操作

### R8 Client 切换

- 在配置面板中选择不同的 .client 文件
- 切换后需要重启引擎（Stop → 更新 config.json → Start）
- 显示当前 client 的 User-Agent 信息

### R9 关窗退出

- 窗口关闭时调用 `SeedManager::stop()` 优雅关闭引擎
- 确保所有 tokio 任务在进程退出前完成

### R10 稳定性

- Windows 10 本机 `cargo run --release` 能启动并稳定交互 ≥ 30 分钟
- 无内存泄漏（speed_history / log_buffer 有上限）
- 无 panic（所有 channel recv 错误优雅处理）

## Non-Goals (this task)

- 多语言 / 主题切换
- --no-gui headless 模式（已有 MVP-1 CLI）
- WebSocket 远程 UI（Java 版的 STOMP 架构不再需要）

## Acceptance Criteria

- [ ] `joal-app` Cargo.toml 添加 eframe / egui / egui_extras / egui_plot 依赖
- [ ] `eframe::run_native` 启动窗口，标题 "JOAL Desktop"
- [ ] Torrent 列表实时刷新（snapshot 变化后 < 1s 反映到 UI）
- [ ] 全局状态栏显示 active client + total speed + torrent count
- [ ] egui_plot 速度曲线正常绘制，5 分钟滚动窗口
- [ ] 日志面板显示 EngineEvent 流
- [ ] Start/Stop 按钮控制引擎启停
- [ ] 配置编辑面板可修改并保存 config.json（min/max rate, simultaneous seed, client, ratio target）
- [ ] Client 下拉列表显示所有可用 .client 文件
- [ ] Torrent 删除按钮（移到 archive）
- [ ] Torrent 添加（文件对话框 → 复制到 torrents/）
- [ ] 关窗触发 SeedManager::stop()，进程干净退出
- [ ] `cargo clippy --all-targets` 零警告
- [ ] `cargo test --workspace` 全绿
- [ ] Windows 10 `cargo run --release -- --joal-conf <path>` 稳定运行 30+ 分钟
