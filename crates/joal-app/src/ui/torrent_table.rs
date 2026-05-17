use std::cmp::Ordering;
use std::time::Duration;

use joal_core::snapshot::{EngineSnapshot, TorrentStatus};
use tokio::sync::mpsc;

use super::DeleteConfirmation;
use super::{i18n::Tr, status_bar::format_speed, theme};
use crate::EngineCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SortColumn {
    Name,
    Progress,
    UploadSpeed,
    Uploaded,
    DownloadSpeed,
    Downloaded,
    Seeders,
    Leechers,
    LastAnnounce,
    Health,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortDirection {
    Ascending,
    Descending,
}

impl SortDirection {
    fn toggled(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

pub(super) struct TableState {
    pub search_query: String,
    pub attention_only: bool,
    sort_column: SortColumn,
    sort_direction: SortDirection,
}

impl Default for TableState {
    fn default() -> Self {
        Self {
            search_query: String::new(),
            attention_only: false,
            sort_column: SortColumn::Name,
            sort_direction: SortDirection::Ascending,
        }
    }
}

impl TableState {
    fn toggle_sort(&mut self, column: SortColumn) {
        if self.sort_column == column {
            self.sort_direction = self.sort_direction.toggled();
        } else {
            self.sort_column = column;
            self.sort_direction = default_sort_direction(column);
        }
    }
}

/// Standalone table toolbar (search + attention filter + visible count). Lives
/// in the top panel so the central panel is occupied entirely by table rows.
pub(super) fn toolbar(
    ui: &mut egui::Ui,
    snapshot: &EngineSnapshot,
    table_state: &mut TableState,
    t: &Tr,
) {
    ui.horizontal_wrapped(|ui| {
        let search_response = ui
            .push_id("torrent_table_search", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut table_state.search_query)
                        .id_salt("torrent_table_search")
                        .hint_text(t.search_torrents)
                        .desired_width(240.0),
                )
            })
            .inner;
        if search_response.changed() {
            search_response.request_focus();
        }

        let attention_clicked = theme::tone_button(
            ui,
            "torrent_table_attention_toggle",
            t.attention_only,
            theme::Tone::Warning,
            egui::vec2(140.0, 30.0),
            table_state.attention_only,
        )
        .on_hover_text(t.attention_hint)
        .clicked();
        if attention_clicked {
            table_state.attention_only = !table_state.attention_only;
        }

        // Push the visible-count badge to the far right so the toolbar reads
        // "controls left, counter right" instead of a left-bunched cluster.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            theme::metric(
                ui,
                "visible_row_count",
                "",
                format!(
                    "{}/{}",
                    visible_count(snapshot, table_state),
                    snapshot.torrents.len()
                ),
                theme::Tone::Neutral,
            );
        });
    });
}

#[allow(clippy::too_many_lines)]
pub fn show(
    ui: &mut egui::Ui,
    snapshot: &mut EngineSnapshot,
    pending_delete: &mut Option<DeleteConfirmation>,
    cmd_tx: &mpsc::Sender<EngineCommand>,
    table_state: &mut TableState,
    t: &Tr,
) {
    if snapshot.torrents.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new(t.no_torrents).color(theme::text_secondary()));
        });
        return;
    }

    let visible_indices = visible_torrent_indices(&snapshot.torrents, table_state);
    if visible_indices.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new(t.no_matching_torrents).color(theme::text_secondary()));
        });
        return;
    }

    let text_height = egui::TextStyle::Body
        .resolve(ui.style())
        .size
        .max(ui.spacing().interact_size.y);
    let row_height = (text_height * 2.1).max(36.0);
    let available_height = ui.available_height();

    // Visually quiet the table: kill the inter-column vertical separators by
    // setting `item_spacing.x` to 0 inside cells (the `TableBuilder` itself
    // already does not draw vertical grid lines), and replace the default
    // separator/horizontal-line stroke with a near-invisible divider.
    let style = ui.style_mut();
    style.visuals.widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0, theme::divider_color());
    // Make selection and active state colors a touch softer to remove the
    // chunky outlined look the old palette had.

    egui_extras::TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .vscroll(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .min_scrolled_height(available_height.max(120.0))
        .max_scroll_height(available_height.max(120.0))
        .column(egui_extras::Column::remainder().at_least(220.0)) // Name
        .column(egui_extras::Column::initial(124.0).at_least(120.0)) // Progress
        .column(egui_extras::Column::initial(82.0).at_least(72.0)) // Upload speed
        .column(egui_extras::Column::initial(92.0).at_least(84.0)) // Uploaded
        .column(egui_extras::Column::initial(82.0).at_least(72.0)) // Download speed
        .column(egui_extras::Column::initial(92.0).at_least(84.0)) // Downloaded
        .column(egui_extras::Column::initial(72.0).at_least(64.0)) // Seeders
        .column(egui_extras::Column::initial(72.0).at_least(64.0)) // Leechers
        .column(egui_extras::Column::initial(128.0).at_least(118.0)) // Last announce
        .column(egui_extras::Column::initial(200.0).at_least(180.0)) // Health
        .column(egui_extras::Column::initial(184.0).at_least(168.0)) // Actions
        .header(text_height + 12.0, |mut header| {
            sortable_header(&mut header, table_state, SortColumn::Name, t.col_name);
            sortable_header(
                &mut header,
                table_state,
                SortColumn::Progress,
                t.col_progress,
            );
            sortable_header(
                &mut header,
                table_state,
                SortColumn::UploadSpeed,
                t.col_speed,
            );
            sortable_header(
                &mut header,
                table_state,
                SortColumn::Uploaded,
                t.col_uploaded,
            );
            sortable_header(
                &mut header,
                table_state,
                SortColumn::DownloadSpeed,
                t.col_dl_speed,
            );
            sortable_header(
                &mut header,
                table_state,
                SortColumn::Downloaded,
                t.col_downloaded,
            );
            sortable_header(&mut header, table_state, SortColumn::Seeders, t.col_seeders);
            sortable_header(
                &mut header,
                table_state,
                SortColumn::Leechers,
                t.col_leechers,
            );
            sortable_header(
                &mut header,
                table_state,
                SortColumn::LastAnnounce,
                t.col_last_announce,
            );
            sortable_header(&mut header, table_state, SortColumn::Health, t.col_health);
            header.col(|ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(t.col_actions)
                            .small()
                            .color(theme::text_secondary())
                            .strong(),
                    )
                    .truncate(),
                );
            });
        })
        .body(|body| {
            body.rows(row_height, visible_indices.len(), |mut row| {
                let row_index = row.index();
                let index = visible_indices[row_index];
                let torrent = &mut snapshot.torrents[index];
                row.col(|ui| {
                    cell_scope(ui, row_index, "name", |ui| {
                        ui.vertical(|ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&torrent.name)
                                        .strong()
                                        .color(theme::text_primary()),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&torrent.name);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(short_hash(torrent))
                                        .monospace()
                                        .small()
                                        .color(theme::text_tertiary()),
                                )
                                .truncate(),
                            )
                            .on_hover_text(torrent.info_hash.to_string());
                        });
                    });
                });
                row.col(|ui| {
                    cell_scope(ui, row_index, "progress", |ui| {
                        let progress = progress_fraction(torrent);
                        let tone = if progress >= 1.0 || torrent.initial_completed {
                            theme::Tone::Success
                        } else {
                            theme::Tone::Accent
                        };
                        // Use the tone's strong foreground color for the
                        // progress fill so it stands out, with a soft track
                        // (panel_fill) so the bar reads as a pill, not a
                        // square.
                        ui.add(
                            egui::ProgressBar::new(progress as f32)
                                .desired_width(ui.available_width())
                                .fill(theme::tone_colors(tone).fg)
                                .corner_radius(egui::CornerRadius::same(theme::CR_BADGE))
                                .text(
                                    egui::RichText::new(progress_text(torrent))
                                        .small()
                                        .strong()
                                        .color(theme::text_primary()),
                                ),
                        );
                    });
                });
                row.col(|ui| {
                    cell_scope(ui, row_index, "upload_speed", |ui| {
                        ui.label(
                            egui::RichText::new(format_speed(torrent.current_speed_bps))
                                .color(theme::text_primary()),
                        );
                    });
                });
                row.col(|ui| {
                    cell_scope(ui, row_index, "uploaded", |ui| {
                        ui.label(
                            egui::RichText::new(format_bytes(torrent.uploaded_bytes))
                                .color(theme::text_primary()),
                        );
                    });
                });
                row.col(|ui| {
                    cell_scope(ui, row_index, "download_speed", |ui| {
                        ui.label(
                            egui::RichText::new(format_speed(torrent.current_download_speed_bps))
                                .color(theme::text_secondary()),
                        );
                    });
                });
                row.col(|ui| {
                    cell_scope(ui, row_index, "downloaded", |ui| {
                        ui.label(
                            egui::RichText::new(format_bytes(torrent.downloaded_bytes))
                                .color(theme::text_secondary()),
                        );
                    });
                });
                row.col(|ui| {
                    cell_scope(ui, row_index, "seeders", |ui| {
                        ui.label(
                            egui::RichText::new(opt_u32(torrent.last_known_seeders))
                                .color(theme::text_primary()),
                        );
                    });
                });
                row.col(|ui| {
                    cell_scope(ui, row_index, "leechers", |ui| {
                        ui.label(
                            egui::RichText::new(opt_u32(torrent.last_known_leechers))
                                .color(theme::text_primary()),
                        );
                    });
                });
                row.col(|ui| {
                    cell_scope(ui, row_index, "announce_meta", |ui| {
                        ui.vertical(|ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(last_announce_text(torrent, t))
                                        .color(theme::text_primary()),
                                )
                                .truncate(),
                            );
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(interval_text(torrent, t))
                                        .small()
                                        .color(theme::text_tertiary()),
                                )
                                .truncate(),
                            );
                        });
                    });
                });
                row.col(|ui| {
                    cell_scope(ui, row_index, "health", |ui| {
                        health_cell(ui, row_index, torrent, t);
                    });
                });
                row.col(|ui| {
                    cell_scope(ui, row_index, "actions", |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            let mark_label = if torrent.initial_completed {
                                t.action_marked_complete
                            } else {
                                t.action_mark_complete
                            };
                            let response = theme::tone_button(
                                ui,
                                "mark_completed",
                                mark_label,
                                theme::Tone::Success,
                                egui::vec2(88.0, 24.0),
                                torrent.initial_completed,
                            )
                            .on_hover_text(t.mark_completed_tooltip);
                            if response.clicked() {
                                torrent.initial_completed = !torrent.initial_completed;
                                let _ =
                                    cmd_tx.try_send(EngineCommand::SetTorrentInitialCompleted {
                                        info_hash: torrent.info_hash.clone(),
                                        completed: torrent.initial_completed,
                                    });
                            }

                            if theme::tone_button(
                                ui,
                                "archive_torrent",
                                t.action_archive,
                                theme::Tone::Danger,
                                egui::vec2(68.0, 24.0),
                                false,
                            )
                            .clicked()
                            {
                                *pending_delete = Some(DeleteConfirmation {
                                    info_hash: torrent.info_hash.clone(),
                                    name: torrent.name.clone(),
                                });
                            }
                        });
                    });
                });
            });
        });
}

fn visible_count(snapshot: &EngineSnapshot, table_state: &TableState) -> usize {
    visible_torrent_indices(&snapshot.torrents, table_state).len()
}

fn visible_torrent_indices(torrents: &[TorrentStatus], table_state: &TableState) -> Vec<usize> {
    let query = table_state.search_query.trim().to_lowercase();
    let mut visible = torrents
        .iter()
        .enumerate()
        .filter(|(_, torrent)| matches_search(torrent, &query))
        .filter(|(_, torrent)| !table_state.attention_only || needs_attention(torrent))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    visible.sort_by(|left, right| {
        compare_torrents(
            &torrents[*left],
            &torrents[*right],
            table_state.sort_column,
            table_state.sort_direction,
        )
    });
    visible
}

fn matches_search(torrent: &TorrentStatus, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    torrent.name.to_lowercase().contains(query) || torrent.info_hash.to_string().contains(query)
}

fn needs_attention(torrent: &TorrentStatus) -> bool {
    torrent.consecutive_fails > 0
        || torrent.last_known_leechers == Some(0)
        || torrent.last_announced_at.is_none()
}

fn compare_torrents(
    left: &TorrentStatus,
    right: &TorrentStatus,
    column: SortColumn,
    direction: SortDirection,
) -> Ordering {
    let ordering = match column {
        SortColumn::Name => left.name.to_lowercase().cmp(&right.name.to_lowercase()),
        SortColumn::Progress => progress_fraction(left).total_cmp(&progress_fraction(right)),
        SortColumn::UploadSpeed => left.current_speed_bps.cmp(&right.current_speed_bps),
        SortColumn::Uploaded => left.uploaded_bytes.cmp(&right.uploaded_bytes),
        SortColumn::DownloadSpeed => left
            .current_download_speed_bps
            .cmp(&right.current_download_speed_bps),
        SortColumn::Downloaded => left.downloaded_bytes.cmp(&right.downloaded_bytes),
        SortColumn::Seeders => left
            .last_known_seeders
            .unwrap_or_default()
            .cmp(&right.last_known_seeders.unwrap_or_default()),
        SortColumn::Leechers => left
            .last_known_leechers
            .unwrap_or_default()
            .cmp(&right.last_known_leechers.unwrap_or_default()),
        SortColumn::LastAnnounce => left.last_announced_at.cmp(&right.last_announced_at),
        SortColumn::Health => health_sort_key(left).cmp(&health_sort_key(right)),
    };

    match direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
    }
    .then_with(|| left.name.cmp(&right.name))
}

fn default_sort_direction(column: SortColumn) -> SortDirection {
    match column {
        SortColumn::Name => SortDirection::Ascending,
        SortColumn::Progress
        | SortColumn::UploadSpeed
        | SortColumn::Uploaded
        | SortColumn::DownloadSpeed
        | SortColumn::Downloaded
        | SortColumn::Seeders
        | SortColumn::Leechers
        | SortColumn::LastAnnounce
        | SortColumn::Health => SortDirection::Descending,
    }
}

fn health_sort_key(torrent: &TorrentStatus) -> (u8, u32, u32) {
    let severity = if torrent.consecutive_fails > 3 {
        4
    } else if torrent.consecutive_fails > 0 {
        3
    } else if torrent.last_known_leechers == Some(0) {
        2
    } else {
        u8::from(torrent.last_announced_at.is_none())
    };
    (
        severity,
        torrent.consecutive_fails,
        torrent.last_known_leechers.unwrap_or_default(),
    )
}

fn sortable_header(
    header: &mut egui_extras::TableRow<'_, '_>,
    table_state: &mut TableState,
    column: SortColumn,
    label: &str,
) {
    header.col(|ui| {
        let active = table_state.sort_column == column;
        let arrow = if active {
            match table_state.sort_direction {
                SortDirection::Ascending => " ↑",
                SortDirection::Descending => " ↓",
            }
        } else {
            ""
        };
        let button_label = format!("{label}{arrow}");
        // Headers are intentionally chrome-free — just colored text. They are
        // still clickable to toggle sort. Use a borderless, fill-less button so
        // the table feels less like a spreadsheet.
        let min_width = (ui.available_width() - 4.0).max(64.0);
        let clicked = ui
            .push_id(("torrent_table_sort", column), |ui| {
                let fg = if active {
                    theme::primary_color()
                } else {
                    theme::text_secondary()
                };
                ui.add(
                    egui::Button::new(
                        egui::RichText::new(button_label)
                            .small()
                            .strong()
                            .color(fg),
                    )
                    .truncate()
                    .fill(egui::Color32::TRANSPARENT)
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(egui::CornerRadius::same(theme::CR_BADGE))
                    .frame_when_inactive(false)
                    .min_size(egui::vec2(min_width, 22.0)),
                )
                .clicked()
            })
            .inner;
        if clicked {
            table_state.toggle_sort(column);
        }
    });
}

fn health_cell(ui: &mut egui::Ui, row_index: usize, torrent: &TorrentStatus, t: &Tr) {
    let (label, tone) = if torrent.consecutive_fails > 3 {
        (t.status_tracker_error, theme::Tone::Danger)
    } else if torrent.consecutive_fails > 0 {
        (t.status_tracker_warning, theme::Tone::Warning)
    } else if torrent.last_known_leechers == Some(0) {
        (t.status_zero_leechers, theme::Tone::Warning)
    } else if torrent.last_announced_at.is_none() {
        (t.status_pending_announce, theme::Tone::Info)
    } else {
        (t.status_healthy, theme::Tone::Success)
    };

    ui.vertical(|ui| {
        theme::badge(ui, (row_index, "health_badge"), label, tone);
        ui.add(
            egui::Label::new(
                egui::RichText::new(health_detail(torrent, t))
                    .small()
                    .color(theme::text_tertiary()),
            )
            .truncate(),
        )
        .on_hover_text(health_detail(torrent, t));
    });
}

fn health_detail(torrent: &TorrentStatus, t: &Tr) -> String {
    let last_announce = format!(
        "{} {}",
        t.status_last_announce,
        last_announce_text(torrent, t)
    );
    if torrent.consecutive_fails > 0 {
        format!(
            "{last_announce} | {} {}",
            torrent.consecutive_fails, t.status_tracker_warning
        )
    } else {
        format!("{last_announce} | {}", interval_text(torrent, t))
    }
}

fn short_hash(torrent: &TorrentStatus) -> String {
    let hash = torrent.info_hash.to_string();
    format!("{}...", &hash[..12])
}

fn last_announce_text(torrent: &TorrentStatus, t: &Tr) -> String {
    match torrent.last_announced_at {
        Some(last_announced_at) => format_duration(last_announced_at.elapsed()),
        None => t.status_never.to_owned(),
    }
}

fn interval_text(torrent: &TorrentStatus, t: &Tr) -> String {
    match torrent.last_known_interval {
        Some(interval) => format!(
            "{} {}",
            t.status_interval,
            format_duration(Duration::from_secs(interval.into()))
        ),
        None => format!("{} {}", t.status_interval, t.status_never),
    }
}

fn format_duration(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    if total_secs < 60 {
        format!("{total_secs}s")
    } else if total_secs < 3600 {
        format!("{}m {}s", total_secs / 60, total_secs % 60)
    } else {
        format!("{}h {}m", total_secs / 3600, (total_secs % 3600) / 60)
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.2} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn opt_u32(val: Option<u32>) -> String {
    val.map_or_else(|| "\u{2014}".to_owned(), |v| v.to_string())
}

fn progress_fraction(torrent: &TorrentStatus) -> f64 {
    if torrent.total_size == 0 {
        1.0
    } else {
        (torrent.downloaded_bytes as f64 / torrent.total_size as f64).clamp(0.0, 1.0)
    }
}

fn progress_text(torrent: &TorrentStatus) -> String {
    format!("{:.1}%", progress_fraction(torrent) * 100.0)
}

fn cell_scope<R>(
    ui: &mut egui::Ui,
    row_index: usize,
    key: &'static str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.push_id((row_index, key), add_contents).inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::i18n::{Language, tr};
    use joal_core::torrent::InfoHash;

    fn torrent(name: &str, fill: u8) -> TorrentStatus {
        let downloaded_bytes = u64::from(fill) * 10;
        TorrentStatus {
            info_hash: InfoHash::from_bytes([fill; 20]),
            name: name.to_owned(),
            total_size: 1000,
            uploaded_bytes: u64::from(fill) * 1000,
            downloaded_bytes,
            left_bytes: 1000 - downloaded_bytes,
            current_speed_bps: u64::from(fill) * 100,
            current_download_speed_bps: u64::from(fill) * 10,
            initial_completed: false,
            last_known_interval: Some(1800),
            last_known_seeders: Some(u32::from(fill)),
            last_known_leechers: Some(u32::from(fill)),
            consecutive_fails: 0,
            last_announced_at: Some(std::time::Instant::now()),
        }
    }

    #[test]
    fn visible_indices_filter_by_search_name_and_hash() {
        let torrents = vec![torrent("Alpha", 0x11), torrent("Beta", 0x22)];
        let mut state = TableState {
            search_query: "alpha".to_owned(),
            ..TableState::default()
        };
        assert_eq!(visible_torrent_indices(&torrents, &state), vec![0]);

        state.search_query = "2222".to_owned();
        assert_eq!(visible_torrent_indices(&torrents, &state), vec![1]);
    }

    #[test]
    fn visible_indices_filter_attention_rows() {
        let mut healthy = torrent("Healthy", 0x11);
        healthy.last_known_leechers = Some(5);

        let mut zero_leechers = torrent("Zero", 0x22);
        zero_leechers.last_known_leechers = Some(0);

        let mut warning = torrent("Warn", 0x33);
        warning.consecutive_fails = 1;

        let state = TableState {
            attention_only: true,
            ..TableState::default()
        };

        let visible = visible_torrent_indices(&[healthy, zero_leechers, warning], &state);
        assert_eq!(visible, vec![2, 1]);
    }

    #[test]
    fn visible_indices_sort_seeders_descending() {
        let low = torrent("Low", 0x11);
        let mut high = torrent("High", 0x22);
        high.last_known_seeders = Some(99);

        let state = TableState {
            sort_column: SortColumn::Seeders,
            sort_direction: SortDirection::Descending,
            ..TableState::default()
        };

        let visible = visible_torrent_indices(&[low, high], &state);
        assert_eq!(visible, vec![1, 0]);
    }

    #[test]
    fn attention_toggle_across_discarded_pass_keeps_widget_ids_stable() {
        let ctx = egui::Context::default();
        let mut healthy = torrent("Healthy", 0x11);
        healthy.last_known_leechers = Some(5);

        let mut warning = torrent("Warn", 0x22);
        warning.consecutive_fails = 1;

        let mut snapshot = EngineSnapshot {
            active_client_filename: "utorrent-3.5.0_43916.client".to_owned(),
            global_upload_speed_bps: 0,
            global_download_speed_bps: 0,
            torrents: vec![healthy, warning],
        };
        let mut pending_delete = None;
        let (cmd_tx, _cmd_rx) = mpsc::channel(8);
        let mut table_state = TableState::default();
        let mut first_pass = true;
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 720.0),
            )),
            ..Default::default()
        };

        let output = ctx.run_ui(raw_input, |ui| {
            show(
                ui,
                &mut snapshot,
                &mut pending_delete,
                &cmd_tx,
                &mut table_state,
                tr(Language::Chinese),
            );
            if first_pass {
                first_pass = false;
                table_state.attention_only = true;
                ui.ctx().request_discard("apply attention filter");
            }
        });

        assert!(
            !contains_id_warning_shape(&output.shapes),
            "expected no debug warning shapes after a discarded-pass filter change",
        );
    }

    #[test]
    fn data_update_across_discarded_pass_keeps_widget_ids_stable() {
        // Reproduces the runtime warning class from `wrong.txt`: a tracker
        // announce returns new seeders / leechers / speed values, the egui
        // multi-pass layout re-runs, and the row widgets must keep stable ids
        // even though their displayed text changed character count between
        // passes (e.g. "0 B/s" -> "120.5 KB/s").
        let ctx = egui::Context::default();
        let mut alpha = torrent("Alpha", 0x11);
        alpha.last_known_seeders = Some(5);
        alpha.last_known_leechers = Some(3);
        alpha.current_speed_bps = 0;

        let mut beta = torrent("Beta", 0x22);
        beta.consecutive_fails = 1;
        beta.last_known_seeders = Some(2);
        beta.last_known_leechers = Some(0);
        beta.current_speed_bps = 1024;

        let mut snapshot = EngineSnapshot {
            active_client_filename: "utorrent-3.5.0_43916.client".to_owned(),
            global_upload_speed_bps: 0,
            global_download_speed_bps: 0,
            torrents: vec![alpha, beta],
        };
        let mut pending_delete = None;
        let (cmd_tx, _cmd_rx) = mpsc::channel(8);
        let mut table_state = TableState::default();
        let mut first_pass = true;
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 720.0),
            )),
            ..Default::default()
        };

        let output = ctx.run_ui(raw_input, |ui| {
            show(
                ui,
                &mut snapshot,
                &mut pending_delete,
                &cmd_tx,
                &mut table_state,
                tr(Language::Chinese),
            );
            if first_pass {
                first_pass = false;
                // Simulate a tracker update arriving between passes — speeds
                // and seeder/leecher counts mutate so several cell label
                // widths shift, but the row slot ids must stay anchored.
                for torrent in &mut snapshot.torrents {
                    torrent.current_speed_bps = torrent.current_speed_bps.saturating_add(120_000);
                    torrent.current_download_speed_bps =
                        torrent.current_download_speed_bps.saturating_add(35_000);
                    torrent.uploaded_bytes = torrent.uploaded_bytes.saturating_add(2_500_000);
                    torrent.downloaded_bytes = torrent.downloaded_bytes.saturating_add(1_200_000);
                    torrent.last_known_seeders =
                        Some(torrent.last_known_seeders.unwrap_or(0).saturating_add(4));
                    torrent.last_known_leechers =
                        Some(torrent.last_known_leechers.unwrap_or(0).saturating_add(7));
                }
                ui.ctx().request_discard("simulate snapshot update");
            }
        });

        assert!(
            !contains_id_warning_shape(&output.shapes),
            "expected no debug warning shapes after a discarded-pass data update",
        );
    }

    fn contains_id_warning_shape(shapes: &[egui::epaint::ClippedShape]) -> bool {
        shapes
            .iter()
            .any(|clipped| shape_contains_id_warning(&clipped.shape))
    }

    fn shape_contains_id_warning(shape: &egui::epaint::Shape) -> bool {
        match shape {
            egui::epaint::Shape::Rect(rect) => {
                rect.fill == egui::Color32::TRANSPARENT
                    && rect.stroke.color == egui::Color32::RED
                    && (rect.stroke.width - 2.0).abs() < f32::EPSILON
            }
            egui::epaint::Shape::Vec(shapes) => shapes.iter().any(shape_contains_id_warning),
            _ => false,
        }
    }
}
