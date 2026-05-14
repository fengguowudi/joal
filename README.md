# JOAL — Jack of All Trades

JOAL 是一个 BitTorrent 做种模拟器。它通过模拟真实 BT 客户端的 announce 行为（peer-id、key、User-Agent、query 格式等），向 tracker 汇报虚拟的上传量，从而在不实际传输数据的情况下维持做种比率。

本项目为 Rust 重写版本，使用 egui 原生桌面 GUI 替代了原 Java + Spring WebSocket 架构。

## 功能特性

- 模拟 90+ 种 BT 客户端（qBittorrent、Deluge、uTorrent、Transmission 等）
- 随机带宽分配，基于 peers 权重的上传速度模拟
- 原生桌面 GUI（egui），支持中英双语
- 实时 torrent 状态监控、速度曲线图、日志面板
- 配置热编辑（上传速率、同时做种数、客户端切换）
- Torrent 文件增删管理
- 文件系统 watcher 自动感知 torrent 变化

## 项目结构

```
joal/
├── Cargo.toml              # workspace 根配置
├── Cargo.lock
├── rust-toolchain.toml     # Rust 工具链声明 (stable)
├── resources/              # 运行时配置目录 (--joal-conf 指向此处)
│   ├── config.json         # 用户配置 (速率/做种数/客户端选择)
│   ├── clients/            # 90 个 .client 文件 (BT 客户端模拟定义)
│   └── torrents/           # 放入 .torrent 文件的目录
│       └── archived/       # 已删除 torrent 的归档
└── crates/
    ├── joal-core/          # 核心引擎库
    │   ├── src/
    │   │   ├── seed_manager.rs    # 组合根，启动/停止引擎
    │   │   ├── snapshot.rs        # EngineSnapshot 状态投影
    │   │   ├── events.rs          # EngineEvent 事件定义
    │   │   ├── config.rs          # 配置加载/保存
    │   │   ├── announcer/         # HTTP tracker announce 逻辑
    │   │   ├── bandwidth/         # 带宽分配与速度模拟
    │   │   ├── client/            # BT 客户端模拟 (peer-id/key 生成器)
    │   │   ├── torrent/           # torrent 解析与文件 watcher
    │   │   └── ttorrent_client/   # announce 调度执行器
    │   └── tests/                 # 集成测试
    ├── joal-app/           # 桌面应用 (egui GUI)
    │   └── src/
    │       ├── main.rs            # 入口：tokio runtime + eframe 启动
    │       └── ui/                # UI 模块
    │           ├── mod.rs         # JoalApp 主结构 + eframe::App 实现
    │           ├── torrent_table.rs  # torrent 列表表格
    │           ├── speed_chart.rs    # 实时速度曲线 (egui_plot)
    │           ├── log_panel.rs      # 事件日志面板
    │           ├── config_panel.rs   # 配置编辑侧面板
    │           ├── status_bar.rs     # 顶部/底部状态栏
    │           └── i18n.rs           # 中英文国际化
    └── joal-testing/       # 测试辅助 crate (共享 fixtures)
```

## 编译环境要求

- **Rust**: stable (通过 `rust-toolchain.toml` 自动管理)
- **操作系统**: Windows 10+、Linux、macOS
- **系统依赖** (Linux):
  ```bash
  # Ubuntu/Debian — egui 的 OpenGL 渲染需要
  sudo apt install libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
       libxkbcommon-dev libssl-dev pkg-config
  ```
- **系统依赖** (Windows/macOS): 无额外依赖

## 编译与运行

### 1. 克隆仓库

```bash
git clone https://github.com/anthonyraymond/joal.git
cd joal
```

### 2. 编译

```bash
# Debug 模式 (快速编译，适合开发)
cargo build

# Release 模式 (优化编译，适合日常使用)
cargo build --release
```

编译产物位于：
- Debug: `target/debug/joal-app`（或 `joal-app.exe`）
- Release: `target/release/joal-app`（或 `joal-app.exe`）

### 3. 运行

```bash
# 使用仓库自带的 resources 目录作为配置
cargo run --release -- --joal-conf ./resources

# 或直接运行编译好的二进制
./target/release/joal-app --joal-conf /path/to/your/joal-conf
```

`--joal-conf` 目录必须包含：
- `config.json` — 运行配置
- `clients/` — 至少一个 `.client` 文件
- `torrents/` — 放入要做种的 `.torrent` 文件

### 4. 测试

```bash
# 运行所有测试
cargo test --workspace

# 运行 clippy 检查
cargo clippy --all-targets
```

## 配置说明

`config.json` 字段：

| 字段 | 类型 | 说明 |
|------|------|------|
| `minUploadRate` | u64 | 最小上传速率 (kB/s) |
| `maxUploadRate` | u64 | 最大上传速率 (kB/s) |
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
│           ├─ spawn tokio runtime on background thread│
│           ├─ SeedManager::start() on that runtime   │
│           └─ eframe::run_native(JoalApp)            │
│                                                     │
│  JoalApp (eframe::App)                              │
│    ├─ snapshot_rx: watch::Receiver<EngineSnapshot>  │
│    ├─ events_rx: broadcast::Receiver<EngineEvent>   │
│    ├─ cmd_tx → EngineCommand channel → tokio runtime│
│    └─ update() → poll snapshot + events → render    │
└─────────────────────────────────────────────────────┘
```

- **tokio runtime** 在独立 OS 线程运行，负责所有异步 I/O（HTTP announce、文件 watcher）
- **eframe** 占据主线程事件循环，每帧非阻塞 poll 状态变化
- 两者通过 `watch`/`broadcast`/`mpsc` channel 通信

## License

MIT
