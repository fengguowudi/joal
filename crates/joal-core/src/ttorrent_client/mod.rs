//! Orchestrator layer (Java: `core.ttorrent.client`).
//!
//! Pieces S9 brings together:
//!
//! | Module | Java counterpart |
//! |--------|------------------|
//! | [`delay_queue`] | `DelayQueue` |
//! | [`announcer_executor`] | `AnnouncerExecutor` |
//! | [`response_handlers`] | `AnnounceResponseHandlerChain` + concrete handlers |
//! | [`announcer_factory`] | `AnnouncerFactory` |
//! | [`client`] | `Client` + `ClientBuilder` |

pub mod announcer_executor;
pub mod announcer_factory;
pub mod client;
pub mod delay_queue;
pub mod response_handlers;

pub use announcer_executor::{AnnounceResponseCallback, AnnouncerExecutor, OrchestratorControl};
pub use announcer_factory::AnnouncerFactory;
pub use client::{ClientError, ClientOrchestrator, ORCHESTRATOR_TICK};
pub use delay_queue::{DelayQueue, InfoHashAble};
pub use response_handlers::{
    AnnounceEventPublisher, AnnounceOutcome, AnnounceReEnqueuer, AnnounceResponseHandler,
    AnnounceResponseHandlerChain, BandwidthDispatcherNotifier, ClientNotifier, MergerPokeNotifier,
};
