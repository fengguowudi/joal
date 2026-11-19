use std::collections::VecDeque;
use std::time::{Duration, Instant};

use joal_core::config::AppConfiguration;
use joal_core::events::EngineEvent;
use joal_core::snapshot::EngineSnapshot;

use crate::engine::{EngineErrorKind, EnginePhase};

use super::i18n::Tr;

const LOG_BUFFER_CAPACITY: usize = 500;
const SPEED_HISTORY_CAPACITY: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct SpeedSample {
    pub t: f64,
    pub up: f64,
    pub down: f64,
}

pub(super) enum LogKind {
    Event(EngineEvent),
    Lagged(u64),
    Error {
        kind: EngineErrorKind,
        detail: String,
    },
    DroppedFiles(usize),
}

pub(super) struct LogEntry {
    pub at: Duration,
    pub kind: LogKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum LogLevel {
    Info,
    Success,
    Warning,
    Error,
}

impl LogEntry {
    pub fn level(&self) -> LogLevel {
        match &self.kind {
            LogKind::Event(
                EngineEvent::AnnounceFailed { .. }
                | EngineEvent::FailedToAddTorrentFile { .. }
                | EngineEvent::TooManyAnnouncesFailedInARow { .. },
            )
            | LogKind::Error { .. } => LogLevel::Error,
            LogKind::Lagged(_) | LogKind::DroppedFiles(_) => LogLevel::Warning,
            LogKind::Event(
                EngineEvent::AnnounceSucceeded { .. }
                | EngineEvent::TorrentFileAdded { .. }
                | EngineEvent::GlobalSeedStarted { .. },
            ) => LogLevel::Success,
            LogKind::Event(_) => LogLevel::Info,
        }
    }

    pub fn text(&self, t: &Tr) -> String {
        match &self.kind {
            LogKind::Event(event) => format_event(event, t),
            LogKind::Lagged(count) => t.log_events_dropped.replace("{count}", &count.to_string()),
            LogKind::DroppedFiles(count) => t.drop_ignored.replace("{count}", &count.to_string()),
            LogKind::Error { kind, detail } => {
                let label = error_kind_text(*kind, t);
                if detail.is_empty() {
                    label.to_owned()
                } else {
                    format!("{label}: {detail}")
                }
            }
        }
    }
}

pub(super) struct Telemetry {
    pub snapshot: EngineSnapshot,
    pub phase: EnginePhase,
    pub logs: VecDeque<LogEntry>,
    pub speed_history: VecDeque<SpeedSample>,
    pub started_at: Instant,
    pub download_enabled: bool,
    pub total_uploaded: u64,
    pub log_auto_scroll: bool,
    pub problems_only: bool,
    last_sample_at: Option<Instant>,
}

impl Telemetry {
    pub fn new(snapshot: EngineSnapshot, config: Option<&AppConfiguration>) -> Self {
        Self {
            snapshot,
            phase: EnginePhase::Starting,
            logs: VecDeque::with_capacity(LOG_BUFFER_CAPACITY),
            speed_history: VecDeque::with_capacity(SPEED_HISTORY_CAPACITY),
            started_at: Instant::now(),
            download_enabled: config.is_some_and(|config| config.max_download_rate > 0),
            total_uploaded: 0,
            log_auto_scroll: true,
            problems_only: false,
            last_sample_at: None,
        }
    }

    pub fn on_snapshot(&mut self, snapshot: EngineSnapshot) {
        self.on_snapshot_at(snapshot, Instant::now());
    }

    fn on_snapshot_at(&mut self, snapshot: EngineSnapshot, now: Instant) {
        self.total_uploaded = snapshot.torrents.iter().fold(0_u64, |total, torrent| {
            total.saturating_add(torrent.uploaded_bytes)
        });
        self.snapshot = snapshot;
        self.push_speed_sample(now);
    }

    fn push_speed_sample(&mut self, now: Instant) {
        let sample = SpeedSample {
            t: now.duration_since(self.started_at).as_secs_f64(),
            up: self.snapshot.global_upload_speed_bps as f64,
            down: self.snapshot.global_download_speed_bps as f64,
        };
        if self
            .last_sample_at
            .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1))
        {
            if let Some(last) = self.speed_history.back_mut() {
                *last = sample;
            }
            return;
        }
        self.speed_history.push_back(sample);
        self.last_sample_at = Some(now);
        if self.speed_history.len() > SPEED_HISTORY_CAPACITY {
            self.speed_history.pop_front();
        }
    }

    pub fn on_phase(&mut self, phase: EnginePhase) {
        self.phase = phase;
        if phase == EnginePhase::Stopped {
            let last_is_zero = self
                .speed_history
                .back()
                .is_some_and(|sample| sample.up == 0.0 && sample.down == 0.0);
            if !last_is_zero {
                self.snapshot.global_upload_speed_bps = 0;
                self.snapshot.global_download_speed_bps = 0;
                self.push_speed_sample(Instant::now());
            }
        }
    }

    pub fn on_event(&mut self, event: EngineEvent) {
        if let EngineEvent::ConfigLoaded { config } = &event {
            self.download_enabled = config.max_download_rate > 0;
        }
        self.push_log(LogKind::Event(event));
    }

    pub fn on_error(&mut self, kind: EngineErrorKind, detail: String) {
        self.push_log(LogKind::Error { kind, detail });
    }

    pub fn on_lagged(&mut self, count: u64) {
        self.push_log(LogKind::Lagged(count));
    }
    pub fn on_dropped_files(&mut self, count: usize) {
        self.push_log(LogKind::DroppedFiles(count));
    }

    fn push_log(&mut self, kind: LogKind) {
        self.logs.push_back(LogEntry {
            at: self.started_at.elapsed(),
            kind,
        });
        if self.logs.len() > LOG_BUFFER_CAPACITY {
            self.logs.pop_front();
        }
    }
}

pub(super) fn error_kind_text(kind: EngineErrorKind, t: &Tr) -> &'static str {
    match kind {
        EngineErrorKind::Start => t.err_start,
        EngineErrorKind::SaveConfig => t.err_save_config,
        EngineErrorKind::RestartAfterConfig => t.err_restart_after_config,
        EngineErrorKind::ListClients => t.err_list_clients,
        EngineErrorKind::AddTorrent => t.err_add_torrent,
        EngineErrorKind::ArchiveTorrent => t.err_archive_torrent,
        EngineErrorKind::SaveTorrentState => t.err_save_torrent_state,
        EngineErrorKind::NotRunning => t.err_not_running,
    }
}

fn format_event(event: &EngineEvent, t: &Tr) -> String {
    match event {
        EngineEvent::GlobalSeedStarted { client_name } => {
            format!("{} - {}: {client_name}", t.log_seeding_started, t.client)
        }
        EngineEvent::GlobalSeedStopped => t.log_seeding_stopped.to_owned(),
        EngineEvent::TorrentFileAdded { name, .. } => {
            format!("{}: {name}", t.log_torrent_added)
        }
        EngineEvent::TorrentFileDeleted { name, .. } => {
            format!("{}: {name}", t.log_torrent_removed)
        }
        EngineEvent::FailedToAddTorrentFile { name, reason } => {
            format!("{}: {name}: {reason}", t.log_add_failed)
        }
        EngineEvent::TooManyAnnouncesFailedInARow { name, .. } => {
            format!("{}: {name}", t.log_too_many_failures)
        }
        EngineEvent::AnnounceStarted {
            name, tracker_url, ..
        } => format!("{}: {name} -> {tracker_url}", t.log_announcing),
        EngineEvent::AnnounceSucceeded {
            name,
            seeders,
            leechers,
            interval,
            ..
        } => format!(
            "{}: {name} (S:{seeders} L:{leechers} I:{interval}s)",
            t.log_announce_ok
        ),
        EngineEvent::AnnounceFailed { name, error, .. } => {
            format!("{}: {name}: {error}", t.log_announce_failed)
        }
        EngineEvent::ConfigLoaded { config } => format!(
            "{} - {}: {}, {} {}-{} kB/s, {} {}-{} kB/s",
            t.log_config_loaded,
            t.client,
            config.client,
            t.upload_kbs,
            config.min_upload_rate,
            config.max_upload_rate,
            t.download_kbs,
            config.min_download_rate,
            config.max_download_rate
        ),
    }
}

#[cfg(test)]
mod tests {
    use joal_core::torrent::InfoHash;

    use super::*;

    #[test]
    fn log_buffer_is_bounded() {
        let mut telemetry = Telemetry::new(EngineSnapshot::default(), None);
        for _ in 0..510 {
            telemetry.on_event(EngineEvent::GlobalSeedStopped);
        }
        assert_eq!(telemetry.logs.len(), LOG_BUFFER_CAPACITY);
    }

    #[test]
    fn speed_samples_use_one_second_windows() {
        let mut telemetry = Telemetry::new(EngineSnapshot::default(), None);
        let now = telemetry.started_at;
        telemetry.on_snapshot_at(
            EngineSnapshot {
                global_upload_speed_bps: 10,
                ..EngineSnapshot::default()
            },
            now,
        );
        telemetry.on_snapshot_at(
            EngineSnapshot {
                global_upload_speed_bps: 20,
                ..EngineSnapshot::default()
            },
            now + Duration::from_millis(200),
        );
        assert_eq!(telemetry.speed_history.len(), 1);
        assert_eq!(telemetry.speed_history[0].up, 20.0);
        telemetry.on_snapshot_at(EngineSnapshot::default(), now + Duration::from_millis(1200));
        assert_eq!(telemetry.speed_history.len(), 2);
    }

    #[test]
    fn log_levels_cover_failures_and_lag() {
        let failure = LogEntry {
            at: Duration::ZERO,
            kind: LogKind::Event(EngineEvent::AnnounceFailed {
                info_hash: InfoHash::from_bytes([0; 20]),
                name: "one".into(),
                error: "failed".into(),
            }),
        };
        assert_eq!(failure.level(), LogLevel::Error);
        let lag = LogEntry {
            at: Duration::ZERO,
            kind: LogKind::Lagged(2),
        };
        assert_eq!(lag.level(), LogLevel::Warning);
    }
}
