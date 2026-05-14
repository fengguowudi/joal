//! JOAL core engine.
//!
//! Mirrors the Java `org.araymond.joal.core` package tree: headless domain logic
//! with no UI coupling. The app crate (`joal-app`) wires this library into a
//! `tokio` runtime and an `egui` front-end.
//!
//! Module map (populated incrementally, see task PRD `rust-mvp1-headless-engine`):
//!
//! | Rust module | Java counterpart |
//! |-------------|------------------|
//! | `bencode`   | (new) wraps `serde_bencode` for `.torrent` + tracker response parsing |
//! | `config`    | `core.config` — `AppConfiguration`, `JoalConfigProvider` |
//! | `client`    | `core.client.emulated` — `BitTorrentClient` + generators |
//! | `torrent`   | `core.torrent` — `InfoHash`, `MockedTorrent`, watcher |
//! | `bandwidth` | `core.bandwith` — dispatcher + weight calculators |
//! | `announcer` | `core.ttorrent.client.announcer` — tracker HTTP client |
//! | `seed_manager` | `core.SeedManager` — top-level orchestrator |
//! | `snapshot`  | (new) per-frame projection consumed by CLI/egui via `watch` |

#![forbid(unsafe_code)]

// Module stubs — each is filled in its own S2..S9 step.
pub mod announcer;
pub mod bandwidth;
pub mod bencode;
pub mod client;
pub mod config;
pub mod events;
pub mod seed_manager;
pub mod snapshot;
pub mod torrent;
pub mod ttorrent_client;
