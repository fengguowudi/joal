# Rewrite JOAL in Rust with egui Frontend

## Goal

把 JOAL（Java 11 + Spring Boot 2.7.3 + ttorrent-core 1.5 的 BitTorrent 假做种 / ratio-master 工具）重写为 **Rust 后端 + egui 前端**。保留原核心功能（客户端模拟、做种带宽控制、tracker announce、torrent 监控、config 管理），借助 Rust 新特性（async/await、零成本抽象、Send/Sync 静态并发安全）获得显著性能提升，并顺手修复原项目里的性能隐患和命名错误。

## What I already know

### 原 Java 项目全貌
- **打包**: `jack-of-all-trades` 2.1.38-SNAPSHOT，Maven，Java 11，Spring Boot Parent 2.7.3
- **核心依赖**:
  - `com.turn:ttorrent-core:1.5`（BT 协议库，作者自己在 README 里说是 fork 自 mpetazzoni 的改版）
  - `com.github.mifmif:generex:1.0.2`（按正则生成随机字符串，用于 peer-id / key）
  - Guava 31.1、commons-io、commons-codec、httpcomponents fluent-hc、log4j2、Lombok
- **源码规模**: `src/main/java` 下约 130 个 `.java` 文件
- **核心包结构**（`org.araymond.joal.core.*`）:
  - `bandwith/`: `BandwidthDispatcher`、`Peers`、`RandomSpeedProvider`、`Speed`、`TorrentSeedStats`、`weight/PeersAwareWeightCalculator`、`weight/WeightHolder` —— 按权重把全局上传带宽分配到各 torrent
  - `client/emulated/`: `BitTorrentClient`、`BitTorrentClientProvider`；generator 子包里有 key/peer-id/numwant 的多种策略（Always/Never/Timed/TorrentVolatile/TorrentPersistent 刷新 + 多种 regex/hash 算法）
  - `config/`: `AppConfiguration`、`JoalConfigProvider`（读写 `config.json`）
  - `events/`: announce/config/global/speed/torrent/files 事件（Spring `ApplicationEvent` 风格）
  - `torrent/`: `InfoHash`、`MockedTorrent`、`TorrentFileProvider`、`watcher/`（文件夹监听）
  - `ttorrent/client/announcer/`: announce 请求发送、response 解析、tracker HTTP 客户端
  - `SeedManager`、`CoreEventListener`
- **web 层**: `org.araymond.joal.web.*`（Spring Boot starter-web + starter-websocket + starter-security + STOMP messaging）—— 没有 REST API，只有 WebSocket STOMP，外加 path prefix + secret token 做"默默无闻的安全"
- **持久化**: 无数据库。`config.json` + `clients/*.client` + `torrents/*.torrent` 放在 joal-conf 文件夹。
- **运行方式**: `java -jar jack-of-all-trades-X.X.X.jar --joal-conf=PATH` 可选 `--spring.main.web-environment=true` 启用 Web UI。
- **已有 spec 参考**: `.trellis/spec/backend/` 已写过 Java 版 directory-structure / error-handling / logging / quality / database (no-db) guidelines，可作为"对照翻译成 Rust 版"的基准。

### `egui-main` 目录真相
- 这是 **egui 框架本体仓库** v0.34.2（workspace，crates 包含 egui / eframe / egui-wgpu / egui-winit / egui_extras 等），不是已经写好的前端。
- Rust edition = **2024**，`rust-version = "1.92"`，`resolver = "2"`。
- 也就是说：Rust 前端需要我们基于 egui（大概率用 eframe）**从零写**，而 `egui-main` 可以作为 path-dep 源码参考/离线依赖，也可以直接从 crates.io 拉同版本。

### 已有约束（从 `.trellis/spec/backend/` 提取）
- 原项目的事件/payload 镜像模式、core/ vs web/ 分层、`@Slf4j` + 参数化日志、`final` everywhere、no-db 文件系统持久化 —— 这些理念要尽量迁移到 Rust 版（tracing、模块分层、不可变数据、serde + 文件系统）。

## Assumptions (temporary, pending confirmation)

1. 用户要的是**桌面单体应用**（Rust 后端逻辑 + egui 原生 GUI 同进程运行），而不是"Rust 守护进程 + 远程 egui 客户端走 WebSocket"。
2. 客户端模拟 / 带宽分配 / tracker announce / torrent watcher / config 的行为必须跟 Java 版**字节级兼容**（peer-id、key、URL 编码、BEP-3 bencode、numwant 策略等都得一致），否则会被 tracker 识破。
3. `config.json`、`clients/*.client`、`torrents/*.torrent` 的现有布局和文件格式保留（用户迁移零成本）。
4. Web UI（STOMP WebSocket + HTML 前端）在重构后不再维持 —— 功能由 egui 原生界面承载。
5. 代码写进仓库根目录的一个新 `rust/` 顶层工作区（与老 Java `src/` 并存一段时间以便对照），不直接把 egui-main 当 workspace root。
6. 目标 Rust 版本对齐 egui v0.34（Rust 1.92+，edition 2024）。
7. 目前先以 Windows 10 本机能跑为准，不优先做 Linux/Docker 镜像（Dockerfile 后续再说）。
8. 性能目标：同等 torrent 数量下内存占用 ≪ Java 版（目标 <100MB 常驻），tracker announce 延迟 p95 下降（得益于 tokio async HTTP），CPU 空闲率显著提高。

## Open Questions (Blocking / Preference only)

（全部已解决）

## Resolved Decisions

- **Q1 架构形态**: **方案 A 单体桌面应用** —— 一个 Rust 可执行文件，tokio 后台 seeding 引擎 + egui 前台窗口，同进程通过 `Arc<RwLock<AppState>>` / `tokio::mpsc` 通信。关闭窗口即停做种；后续如需 headless 再加 `--no-gui` 入口。
- **Q2 BT 协议层**: **自拼最小协议栈** —— `serde_bencode` + `sha1` + `reqwest` + `percent-encoding`，完全掌控 URL 编码顺序 / header / 参数顺序，保证跟 Java 版 ttorrent-core 1.5 字节级一致；不引入 librqbit 等完整 BT 下载器库。
- **Q3 Java 旧码**: **原地保留对照，稳定后统一删除** —— Java 代码留在 `src/` + `pom.xml` + `Dockerfile` 不动，Rust 代码放到根目录新建 `rust/` 工作区；重构期间 Java 源码作为"字节级兼容"的唯一可信 spec，供实现时直接对照；Rust 核心模块全部通过回归测试后用独立清理 PR 批量删除。
- **Q4 egui 依赖方式**: **crates.io 拉 `eframe = "0.34"`** —— 跟本地 `egui-main/` v0.34.2 对齐；本地仓库保留作为示例/文档参考，不作为依赖路径。若未来需修改 egui 源码再改用 `[patch.crates-io]`。
- **Q5 正则生成库**: **`rand_regex`** —— 用于 peer-id / key 的按正则随机生成，替代 Java 版 generex 1.0.2。
- **Q6 MVP 分阶段**: **引擎优先 → UI 次之 → 清理** —— 阶段 1 纯 headless Rust 引擎跑通做种闭环（config → client → torrent → bandwidth → announcer → seed manager），CLI 打印状态并通过与 Java 版抓包对照完成字节级兼容验证；阶段 2 在引擎之上加 egui 窗口（torrent 列表、速度曲线、config 编辑、启停按钮）；阶段 3 删除 Java 代码、更新 README/Docker/release 工作流。

## Requirements (evolving)

- **R1 单进程架构**: seeding 引擎（tokio 后台任务）+ egui 窗口（前台渲染）同进程；前后端经由内存通道通信，不引入 HTTP/WebSocket 层。
- **R2 无头模式预留**: 架构上保留后续加 `--no-gui` CLI 开关的能力（业务核心必须能在没有 egui context 的情况下运行），但本次 MVP 不交付该开关。

## Acceptance Criteria

### 阶段 1 MVP-1: Headless Rust 引擎
- [ ] 根目录新建 `rust/` Cargo workspace（resolver=2, edition=2024, rust 1.92+），与 Java 的 `src/` 并存
- [ ] 能读取现有 `joal-conf/` 目录结构（`config.json` + `clients/*.client` + `torrents/*.torrent`），无迁移成本
- [ ] 实现 client 模拟全链路：BitTorrentClient 加载 + peer-id / key / numwant generator 全部 refresh 策略（Always/Never/Timed/TorrentVolatile/TorrentPersistent）
- [ ] bencode 解析、info_hash 计算、`.torrent` 读取与 Java 版**字节级一致**（测试用例：挑 5 个真实 torrent 文件做对照）
- [ ] tracker HTTP announce URL 构造与 Java 版**字节级一致**（Wireshark 抓包或 mock tracker 对比）
- [ ] bandwidth dispatcher 按权重分配上传速度，周期 tick 更新 stats；速度在 `minUploadRate` / `maxUploadRate` 区间内
- [ ] torrent 文件夹热加载（新增 / 删除 `.torrent` 自动感知）
- [ ] tracker 失败重试 / fallback announce list 行为与 Java 版一致
- [ ] CLI 打印启动参数、加载的 client、torrent 列表、周期性 announce 状态
- [ ] `cargo test --workspace` 全绿；`cargo clippy -D warnings` 零告警
- [ ] 可以用真实 joal-conf 启动并持续做种 ≥ 1 小时无 panic / 资源泄漏

### 阶段 2 MVP-2: egui 前端
- [ ] 主窗口显示 torrent 列表（名称、info_hash 前缀、上传/下载、seeders/leechers、tracker 状态、当前分配速度）
- [ ] 全局控制区：启停做种、切换 client、打开 config.json 编辑面板
- [ ] 实时速度曲线（egui_plot，per-torrent 和 total）
- [ ] 日志面板（tracing_subscriber 桥接到 egui ring buffer）
- [ ] tokio 后台引擎 + egui ctx.request_repaint() 集成，关窗即退出
- [ ] Windows 10 本机 `cargo run --release` 能启动并稳定交互 ≥ 30 分钟

### 阶段 3: 清理
- [ ] 删除 `src/main/java`、`src/test`、`pom.xml`、`Dockerfile`、`publish.sh` 等 Java 相关
- [ ] 更新 `README.md`（中英）说明 Rust 版使用方式
- [ ] `.trellis/spec/backend/` 的 Java 指南归档，新增 `.trellis/spec/backend/` 的 Rust 指南（directory-structure / error-handling / logging / quality / persistence 五篇）

## Definition of Done (team quality bar)

- 原核心功能行为与 Java 版对等（client 模拟/带宽/announce/torrent 管理/config）
- 单元 + 集成测试覆盖关键路径；`cargo fmt` / `cargo clippy -D warnings` / `cargo test` 全绿
- 性能指标：内存占用、announce 吞吐、启动时间有可量化的改善数据
- 新 Rust 项目的 spec（directory-structure / error-handling / logging / quality / persistence）写入 `.trellis/spec/` 对应层
- README 更新 Rust 版本的使用方式

## Technical Approach

### 架构总览

```
┌──────────────────────────────────────────────────────────┐
│  joal-desktop (单个可执行文件)                           │
│  ┌─────────────────┐      ┌─────────────────────────┐   │
│  │  egui UI 线程    │◄────►│ tokio 后台引擎           │   │
│  │  (main thread)   │      │ (multi-thread runtime)   │   │
│  │                 │ mpsc │                         │   │
│  │  - 渲染列表      │◄───► │  SeedManager             │   │
│  │  - 速度曲线      │      │   ├─ ConfigProvider      │   │
│  │  - 按钮事件      │      │   ├─ ClientProvider      │   │
│  └─────────────────┘      │   ├─ TorrentWatcher       │   │
│                           │   ├─ BandwidthDispatcher  │   │
│                           │   └─ AnnouncerPool        │   │
│                           └─────────────────────────┘   │
└──────────────────────────────────────────────────────────┘
                                   │
                        HTTP(S)    │    (reqwest async client)
                                   ▼
                            BitTorrent trackers
```

### Cargo workspace 结构（计划）

```
rust/
├── Cargo.toml                    # workspace root
├── crates/
│   ├── joal-core/                # 无 UI 依赖的核心库
│   │   ├── config/               # config.json + AppConfiguration
│   │   ├── client/               # BitTorrentClient + generators
│   │   ├── torrent/              # InfoHash + MockedTorrent + watcher
│   │   ├── bandwidth/            # BandwidthDispatcher + weight
│   │   ├── announcer/            # tracker HTTP client
│   │   ├── bencode/              # 薄封装 serde_bencode
│   │   └── seed_manager.rs
│   ├── joal-app/                 # eframe/egui 前端 + CLI 入口
│   │   ├── src/main.rs
│   │   ├── src/ui/               # 窗口、列表、曲线
│   │   └── src/bridge.rs         # core ↔ ui channel
│   └── joal-testing/             # 测试夹具、Java 对照样本
└── rust-toolchain.toml
```

### 关键依赖

| 用途 | Crate |
|------|-------|
| 异步运行时 | `tokio` 1.x (rt-multi-thread, macros, sync, fs, time) |
| HTTP 客户端 | `reqwest` 0.12 (rustls-tls, gzip) |
| bencode | `serde_bencode` + `serde_bytes` |
| 哈希 | `sha1` |
| URL 编码 | `percent-encoding` |
| 正则生成 | `rand_regex` + `regex-syntax` + `rand` |
| 文件 watcher | `notify` 7.x |
| 序列化 | `serde` + `serde_json` |
| 日志 | `tracing` + `tracing-subscriber` |
| 错误 | `thiserror`（库）+ `anyhow`（应用） |
| UI | `eframe` 0.34 + `egui_extras` + `egui_plot` |
| CLI 参数 | `clap` 4.x (derive) |

### 关键设计决策

1. **core 不依赖 egui**：`joal-core` 完全是 headless 异步库，`joal-app` 负责 UI 与桥接。为未来 headless daemon 留门。
2. **字节级兼容**：announce URL 构造、peer-id/key 生成、bencode 序列化顺序全部写 Java 对照快照测试。
3. **并发升级**：Java 版的 `synchronized` + `ScheduledExecutorService` 换成 `tokio::sync::RwLock` + `tokio::time::interval`；BandwidthDispatcher 改 actor 风格（单 task owns state，外部用 mpsc 发指令）。
4. **配置热更**: `config.json` 变更通过 `notify` 触发，tokio task reload 后广播 event，避免 Java 版的显式 `@EventListener` 风格。
5. **错误策略**：core 层用 `thiserror` 自定义错误枚举；app 层用 `anyhow` 汇聚。禁止 `unwrap()`（tests 除外）。

## Decision (ADR-lite)

**Context**: JOAL 原为 Java/Spring Boot 实现，资源占用偏高且单进程并发模型受 JVM 线程池约束。用户希望借 Rust 新特性（async、零成本抽象、静态并发安全）显著降低资源开销并提升响应度，同时把 Web UI 换成本地 egui。

**Decision**: 采用"Rust 单体桌面应用"方案（Q1=A, Q2=A, Q3=A, Q4=A, Q5=A, Q6=A），分三阶段交付：
1. headless 核心引擎（字节级兼容 Java）；
2. egui 前端窗口；
3. 清理 Java 代码与文档。

**Consequences**:
- 正面：资源占用大幅下降、协议层完全可控、未来可切 headless 模式；
- 风险：BT 协议层需自己实现（用小规模依赖拼装 + Java 对照测试缓解）；egui + tokio 集成需要注意 `ctx.request_repaint()` 正确性；
- 未做：Docker 打包、跨平台 installer、UDP tracker、DHT（均在 Out of Scope）。

## Out of Scope (explicit)

- 协议层扩展（UDP tracker、DHT、PEX、WebTorrent）——非用户要求，原 Java 版也没做
- 跨平台打包发布（macOS dmg、Linux AppImage、Windows installer）—— 本次先保证能 `cargo run` 起来
- 汉化、多主题等非功能需求
- Docker 镜像 / VPS 服务器部署 —— 桌面应用形态定位
- 原 Web UI（STOMP WebSocket）—— 由 egui 原生界面承载

## Implementation Plan (subtask 分解)

分 3 个子任务，每个子任务独立可交付：

| 子任务 | 目标 | 预估规模 |
|--------|------|----------|
| **rust-mvp1-headless-engine** | Rust workspace + config/client/torrent/bandwidth/announcer/seed_manager 全链路；CLI 入口；与 Java 版字节级对照测试 | 大 |
| **rust-mvp2-egui-frontend** | eframe/egui 窗口 + 列表/曲线/按钮 + tokio↔egui 桥接 | 中 |
| **rust-cleanup-remove-java** | 删 Java 代码 + 更新 README + 迁移 spec | 小 |

子任务会在当前任务 `task.py start` 后、进入实现阶段时按需用 `task.py create --parent` 拆出来。

## Technical Notes

- **egui 库版本**: 本地 `egui-main` 工作区 v0.34.2（edition 2024, rustc 1.92+）仅作参考阅读；依赖从 crates.io 拉取。
- **BT 协议库选型风险点**: Rust 生态缺乏跟 `ttorrent-core` 一对一等价的库，现成的 `librqbit`、`rust-bittorrent` 多聚焦实际下载。JOAL 只需要 bencode 解析、`.torrent` 读取、info_hash 计算、HTTP tracker announce 请求 —— 这些完全可以用 `serde_bencode` + `sha1` + `reqwest` 自己拼。
- **并发模型对 Java 版的升级**: Java 版 `BandwidthDispatcher` 用 `synchronized` + `ScheduledExecutorService`，Rust 改 `tokio::sync::RwLock` + `tokio::time::interval`，或 actor + `mpsc`。
- **Windows 本机环境**: Git Bash shell，`python`（不是 `python3`）；Trellis 脚本用 `python`。
- **字节级兼容验证手段**: 阶段 1 收尾时用同一套 joal-conf 同时跑 Java 版 / Rust 版，用 mitmproxy 或 Wireshark 抓 tracker 请求对比；必要时写 Java 端的样本生成工具把 announce URL 固化到 `testdata/` 供 Rust 单测断言。
- **已有 spec 参考**: `.trellis/spec/backend/` 的 Java 版指南（directory-structure / error-handling / logging / quality / database）作为设计原则的参考，不作为 Rust 代码的强约束。

## Research References

（阶段 1 启动后再由 trellis-research sub-agent 按需补充；本 brainstorm 阶段的决策点均已通过对本地代码和已知生态知识的分析得出，不需要额外 web research 阻塞任务启动。）
