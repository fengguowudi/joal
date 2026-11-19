# JOAL — Jack of All Trades

[![CI](https://github.com/fengguowudi/joal/actions/workflows/ci.yml/badge.svg)](https://github.com/fengguowudi/joal/actions/workflows/ci.yml)

JOAL 是一个 BitTorrent 做种模拟器。它通过模拟真实 BT 客户端的 announce 行为（peer-id、key、User-Agent、query 格式等），向 tracker 汇报虚拟的上传量，从而在不实际传输数据的情况下维持做种比率。

本项目为 Rust 重写版本，使用 egui 原生桌面 GUI 替代了原 Java + Spring WebSocket 架构。

## 下载

前往 [Releases](https://github.com/fengguowudi/joal/releases) 页面下载最新版本：

| 平台 | 文件 |
|------|------|
| Windows x64 | `joal-windows-x86_64.zip` |
| Linux x64 | `joal-linux-x86_64.tar.gz` |
| macOS x64 | `joal-macos-x86_64.tar.gz` |
| macOS ARM | `joal-macos-aarch64.tar.gz` |

下载后解压即可运行，`resources/` 目录已包含在压缩包内。

## 功能特性

- 模拟 90+ 种 BT 客户端（qBittorrent、Deluge、uTorrent、Transmission 等）
- 随机带宽分配，基于 peers 权重的上传速度模拟
- 原生桌面 GUI（egui），支持中英双语
- 实时 torrent 状态监控、速度曲线图、日志面板
- 配置热编辑（上传/下载伪装速率、同时做种数、客户端切换）
- Torrent 文件增删管理
- 文件系统 watcher 自动感知 torrent 变化

## 编译环境要求

- **Rust**: stable (通过 `rust-toolchain.toml` 自动管理)
- **操作系统**: Windows 10+ / Linux / macOS
- **系统依赖**: 
  - **Linux**: `libx11-dev`, `libwayland-dev`（编译时需要）

## 编译与运行

### 克隆仓库

```bash
git clone https://github.com/fengguowudi/joal.git
cd joal
```

### 编译

```bash
# Debug 模式 (快速编译，适合开发)
cargo build

# Release 模式 (优化编译，适合日常使用)
cargo build --release
```

编译产物位于：
- Debug: `target/debug/joal-desktop`（Windows 为 `joal-desktop.exe`）
- Release: `target/release/joal-desktop`（Windows 为 `joal-desktop.exe`）

### 运行

```bash
# 使用仓库自带的 resources 目录作为配置
cargo run --release -p joal-app -- --joal-conf ./resources

# 或直接运行编译好的二进制
./target/release/joal-desktop --joal-conf /path/to/your/joal-conf
```

`--joal-conf` 目录必须包含：
- `config.json` — 运行配置
- `clients/` — 至少一个 `.client` 文件
- `torrents/` — 放入要做种的 `.torrent` 文件

运行过程中，GUI 还会自动维护一个可选文件：
- `torrent_state.json` — per-torrent UI 状态（例如“标记为已完成”），由应用写入，不需要手动编辑

### 测试

```bash
cargo test --workspace
cargo clippy --all-targets
```

## 配置说明

`config.json` 字段：

| 字段 | 类型 | 说明 |
|------|------|------|
| `minUploadRate` | u64 | 最小上传速率 (kB/s) |
| `maxUploadRate` | u64 | 最大上传速率 (kB/s) |
| `minDownloadRate` | u64 | 最小下载伪装速率 (kB/s)，`0` 表示关闭 |
| `maxDownloadRate` | u64 | 最大下载伪装速率 (kB/s)，`0` 表示关闭 |
| `simultaneousSeed` | u32 | 同时做种的 torrent 数量 |
| `client` | string | 使用的客户端文件名 (对应 clients/ 下的文件) |
| `keepTorrentWithZeroLeechers` | bool | 是否保留无下载者的 torrent |
| `uploadRatioTarget` | f32 | 上传比率目标 (-1.0 = 永久做种) |

## 架构概览

```
┌─────────────────────────────────────────────────────┐
│  joal-app (binary)                                  │
│                                                     │
│  main() ─┬─ parse Args (--joal-conf)               │
│           ├─ init tracing                           │
│           ├─ engine::spawn(supervisor)              │
│           └─ eframe::run_native(JoalApp)            │
│                                                     │
│  engine.rs supervisor                               │
│    ├─ 唯一持有 Option<SeedManager>                  │
│    ├─ 稳定 watch<EngineSnapshot> 输出               │
│    └─ mpsc<UiMessage>：阶段、事件、错误、客户端      │
│                                                     │
│  JoalApp (eframe::App)                              │
│    ├─ logic()：收消息 → 更新 Telemetry / 表格模型    │
│    └─ ui()：只读快照 → 绘制 → 发送 EngineCommand     │
└─────────────────────────────────────────────────────┘
```

- **tokio runtime** 在独立 OS 线程运行，负责所有异步 I/O（HTTP announce、文件 watcher）
- 引擎由监督任务在后台启动，与窗口打开并行；启动中、启动失败和重试都通过 UI 阶段消息呈现
- **eframe** 占据主线程事件循环，每帧非阻塞收取快照与监督消息
- 监督任务与 UI 通过稳定的 `watch` / `mpsc` channel 通信，核心事件只在监督任务内从 `broadcast` 转发

## License

MIT
