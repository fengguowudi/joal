#![allow(deprecated)]

use std::collections::VecDeque;
use std::hint::black_box;
use std::mem::size_of;
use std::time::Instant;

use joal_core::config::AppConfiguration;
use joal_core::snapshot::{EngineSnapshot, TorrentStatus};
use joal_core::torrent::InfoHash;
use tokio::sync::mpsc;

use super::config_panel;
use super::i18n::{Language, tr};
use super::{
    ConfigEditState, DeleteConfirmation, LogEntry, log_panel, speed_chart, status_bar, theme,
    torrent_table,
};

struct BenchmarkReport {
    torrent_count: usize,
    approx_workspace_bytes: usize,
    snapshot_clone_avg_us: f64,
    ui_frame_avg_ms: f64,
    avg_shape_count: f64,
}

#[test]
#[ignore = "benchmark harness"]
fn benchmark_default_200_profile() {
    let report = run_default_200_profile();

    println!("default-200 benchmark");
    println!("  torrents: {}", report.torrent_count);
    println!(
        "  approx_workspace_bytes: {} ({:.2} MiB)",
        report.approx_workspace_bytes,
        report.approx_workspace_bytes as f64 / 1_048_576.0,
    );
    println!(
        "  snapshot_clone_avg_us: {:.2}",
        report.snapshot_clone_avg_us,
    );
    println!("  ui_frame_avg_ms: {:.3}", report.ui_frame_avg_ms);
    println!("  avg_shape_count: {:.1}", report.avg_shape_count);
    println!(
        "  available_parallelism: {}",
        std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get),
    );
}

fn run_default_200_profile() -> BenchmarkReport {
    let snapshot = build_snapshot(200);
    let logs = build_logs(180);
    let speed_history = build_speed_history(300);
    let approx_workspace_bytes = estimate_workspace_bytes(&snapshot, &logs, &speed_history);

    let snapshot_clone_avg_us = benchmark_snapshot_clone(&snapshot, 2_000);
    let (ui_frame_avg_ms, avg_shape_count) =
        benchmark_ui_frame(&snapshot, &logs, &speed_history, 180);

    BenchmarkReport {
        torrent_count: snapshot.torrents.len(),
        approx_workspace_bytes,
        snapshot_clone_avg_us,
        ui_frame_avg_ms,
        avg_shape_count,
    }
}

fn build_snapshot(torrent_count: usize) -> EngineSnapshot {
    let active_client = "utorrent-3.5.0_43916.client".to_owned();
    let mut torrents = Vec::with_capacity(torrent_count);
    for index in 0..torrent_count {
        let fill = u8::try_from(index % 255).unwrap_or(0);
        let total_size = 8 * 1024 * 1024 * 1024_u64;
        let downloaded_bytes = (u64::try_from(index).unwrap_or(0) * 31_000_000) % total_size;
        let leechers = if index % 9 == 0 {
            Some(0)
        } else {
            Some(u32::try_from((index % 23) + 1).unwrap_or(1))
        };
        let fails = if index % 17 == 0 {
            4
        } else if index % 11 == 0 {
            2
        } else {
            0
        };
        torrents.push(TorrentStatus {
            info_hash: InfoHash::from_bytes([fill; 20]),
            name: format!("torrent-{index:03}"),
            total_size,
            uploaded_bytes: (u64::try_from(index).unwrap_or(0) + 1) * 2_200_000_000,
            downloaded_bytes,
            left_bytes: total_size.saturating_sub(downloaded_bytes),
            current_speed_bps: (u64::try_from(index).unwrap_or(0) % 25 + 1) * 110_000,
            current_download_speed_bps: (u64::try_from(index).unwrap_or(0) % 18) * 35_000,
            initial_completed: index % 8 == 0,
            last_known_interval: Some(1_800),
            last_known_seeders: Some(u32::try_from((index % 40) + 2).unwrap_or(2)),
            last_known_leechers: leechers,
            consecutive_fails: fails,
            last_announced_at: if index % 13 == 0 {
                None
            } else {
                Some(Instant::now())
            },
        });
    }

    EngineSnapshot {
        active_client_filename: active_client,
        global_upload_speed_bps: torrents
            .iter()
            .map(|torrent| torrent.current_speed_bps)
            .sum(),
        global_download_speed_bps: torrents
            .iter()
            .map(|torrent| torrent.current_download_speed_bps)
            .sum(),
        torrents,
    }
}

fn build_logs(entry_count: usize) -> VecDeque<LogEntry> {
    let mut logs = VecDeque::with_capacity(entry_count);
    for index in 0..entry_count {
        logs.push_back(LogEntry {
            timestamp: Instant::now(),
            message: format!(
                "[info] tracker cycle completed for torrent-{index:03} (seeders={}, leechers={})",
                (index % 40) + 2,
                index % 17,
            ),
        });
    }
    logs
}

fn build_speed_history(point_count: usize) -> VecDeque<(f64, f64)> {
    let mut history = VecDeque::with_capacity(point_count);
    for index in 0..point_count {
        let seconds = index as f64;
        let speed = 1_500_000.0 + (index as f64 % 25.0) * 42_000.0;
        history.push_back((seconds, speed));
    }
    history
}

fn estimate_workspace_bytes(
    snapshot: &EngineSnapshot,
    logs: &VecDeque<LogEntry>,
    speed_history: &VecDeque<(f64, f64)>,
) -> usize {
    let snapshot_bytes = size_of::<EngineSnapshot>()
        + snapshot.active_client_filename.capacity()
        + snapshot.torrents.capacity() * size_of::<TorrentStatus>()
        + snapshot
            .torrents
            .iter()
            .map(|torrent| torrent.name.capacity())
            .sum::<usize>();

    let log_bytes = size_of::<VecDeque<LogEntry>>()
        + logs.capacity() * size_of::<LogEntry>()
        + logs
            .iter()
            .map(|entry| entry.message.capacity())
            .sum::<usize>();

    let speed_bytes =
        size_of::<VecDeque<(f64, f64)>>() + speed_history.capacity() * size_of::<(f64, f64)>();

    snapshot_bytes + log_bytes + speed_bytes
}

fn benchmark_snapshot_clone(snapshot: &EngineSnapshot, iterations: usize) -> f64 {
    let started_at = Instant::now();
    for _ in 0..iterations {
        black_box(snapshot.clone());
    }
    started_at.elapsed().as_secs_f64() * 1_000_000.0 / iterations as f64
}

fn benchmark_ui_frame(
    snapshot: &EngineSnapshot,
    logs: &VecDeque<LogEntry>,
    speed_history: &VecDeque<(f64, f64)>,
    frames: usize,
) -> (f64, f64) {
    let ctx = egui::Context::default();
    theme::apply(&ctx);
    let mut snapshot = snapshot.clone();
    let mut config_edit = ConfigEditState::from_snapshot(
        &snapshot,
        Some(&AppConfiguration {
            min_upload_rate: 30,
            max_upload_rate: 170,
            min_download_rate: 0,
            max_download_rate: 0,
            simultaneous_seed: 200,
            client: snapshot.active_client_filename.clone(),
            keep_torrent_with_zero_leechers: true,
            upload_ratio_target: -1.0,
            proxy_host: None,
            proxy_port: None,
        }),
    );
    let mut table_state = torrent_table::TableState::default();
    let mut log_auto_scroll = false;
    let started_at = Instant::now();
    let t = tr(Language::English);
    let available_clients = vec![snapshot.active_client_filename.clone()];
    let (cmd_tx, _cmd_rx) = mpsc::channel(32);
    let mut pending_delete: Option<DeleteConfirmation> = None;
    let raw_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1600.0, 900.0),
        )),
        predicted_dt: 1.0 / 60.0,
        ..Default::default()
    };

    let benchmark_started_at = Instant::now();
    let mut total_shapes = 0usize;
    for frame_index in 0..frames {
        let mut frame_input = raw_input.clone();
        frame_input.time = Some(frame_index as f64 / 60.0);
        let output = ctx.run(frame_input, |ctx| {
            egui::Panel::top("benchmark_top").show(ctx, |ui| {
                status_bar::top_bar(ui, &snapshot, true, t);
            });
            egui::Panel::bottom("benchmark_bottom").show(ctx, |ui| {
                status_bar::bottom_bar(ui, started_at, true, t);
            });
            egui::Panel::right("benchmark_config")
                .default_size(320.0)
                .min_size(280.0)
                .max_size(460.0)
                .resizable(true)
                .show(ctx, |ui| {
                    let _ = config_panel::show(
                        ui,
                        &mut config_edit,
                        config_panel::ConfigPanelView {
                            validation_errors: &[],
                            operation_error: None,
                            notice: None,
                            apply_in_progress: false,
                            available_clients: &available_clients,
                            t,
                        },
                    );
                });
            egui::Panel::bottom("benchmark_telemetry")
                .default_size(220.0)
                .min_size(160.0)
                .resizable(true)
                .show(ctx, |ui| {
                    egui::Panel::bottom("benchmark_log")
                        .default_size(110.0)
                        .min_size(80.0)
                        .resizable(true)
                        .show_inside(ui, |ui| {
                            log_panel::show(ui, logs, &mut log_auto_scroll, started_at, t);
                        });
                    egui::CentralPanel::default().show_inside(ui, |ui| {
                        speed_chart::show(ui, speed_history, t);
                    });
                });
            egui::CentralPanel::default().show(ctx, |ui| {
                torrent_table::show(
                    ui,
                    &mut snapshot,
                    &mut pending_delete,
                    &cmd_tx,
                    &mut table_state,
                    t,
                );
            });
        });
        total_shapes += output.shapes.len();
        black_box(output.shapes.len());
    }

    (
        benchmark_started_at.elapsed().as_secs_f64() * 1_000.0 / frames as f64,
        total_shapes as f64 / frames as f64,
    )
}
