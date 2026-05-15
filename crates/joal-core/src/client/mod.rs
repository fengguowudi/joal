//! Emulated BitTorrent client: peer-id / key / numwant generators + `.client` loader.
//!
//! Filled in steps **S4–S6**.
//! Mirrors Java `org.araymond.joal.core.client.emulated`.

pub mod bit_torrent_client;
pub mod config;
pub mod error;
pub mod event;
pub mod generator;
pub mod provider;
pub mod runtime;
pub(crate) mod utils;

pub use bit_torrent_client::BitTorrentClient;
pub use config::{BitTorrentClientConfig, HttpHeader};
pub use error::ClientError;
pub use event::RequestEvent;
pub use provider::BitTorrentClientProvider;
pub use runtime::ConnectionHandler;
pub use runtime::{IP_REFRESH_INTERVAL, fetch_public_ip, spawn_ip_refresher};
pub use utils::Casing;
