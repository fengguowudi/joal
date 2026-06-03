use std::cmp::Ordering;

use joal_core::snapshot::{EngineSnapshot, TorrentStatus};
use tokio::sync::mpsc;
use tracing::warn;

use super::DeleteConfirmation;
use super::{i18n::Tr, status_bar::format_speed, theme};
use crate::EngineCommand;
use torrent_table_format::{
    format_bytes, interval_text, last_announce_text, opt_u32, progress_fraction, progress_text,
    short_hash,
};

#[path = "torrent_table_format.rs"]
mod torrent_table_format;

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
                &format!(
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
    style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, theme::divider_color());
    // Make selection and active state colors a touch softer to remove the
    // chunky outlined look the old palette had.

    // Sum of the columns' `at_least` widths. Once the viewport is narrower
    // than this, the table needs horizontal scrolling so the right-edge
    // action cluster does not get clipped behind the viewport edge. Vertical
    // scrolling is still handled by `TableBuilder::vscroll(true)` below — the
    // outer `ScrollArea` here is horizontal-only so we do not double-wrap the
    // same axis. The floors below are deliberately tight: at the default
    // 1180x740 window the user prefers a compact layout and would rather drag
    // the window wider than see columns greedily claim space.
    let min_table_width = 140.0 // Name
        + 120.0 // Progress
        + 72.0 + 84.0 // Upload speed + Uploaded
        + 72.0 + 84.0 // Download speed + Downloaded
        + 44.0 + 44.0 // Seeders + Leechers
        + 108.0 // Last announce
        + 100.0 // Health
        + 168.0 // Actions
        + 8.0; // small slack so the last column does not touch the scrollbar
    egui::ScrollArea::horizontal()
        .id_salt("torrent_table_hscroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(min_table_width);
            TableRender {
                snapshot,
                pending_delete,
                cmd_tx,
                table_state,
                t,
                visible_indices: &visible_indices,
                text_height,
                row_height,
                available_height,
            }
            .show(ui);
        });
}

struct TableRender<'a> {
    snapshot: &'a mut EngineSnapshot,
    pending_delete: &'a mut Option<DeleteConfirmation>,
    cmd_tx: &'a mpsc::Sender<EngineCommand>,
    table_state: &'a mut TableState,
    t: &'a Tr,
    visible_indices: &'a [usize],
    text_height: f32,
    row_height: f32,
    available_height: f32,
}

impl TableRender<'_> {
    fn show(&mut self, ui: &mut egui::Ui) {
        egui_extras::TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .vscroll(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .min_scrolled_height(self.available_height.max(120.0))
            .max_scroll_height(self.available_height.max(120.0))
            .column(
                egui_extras::Column::initial(200.0)
                    .at_least(140.0)
                    .at_most(280.0)
                    .clip(true),
            )
            .column(egui_extras::Column::initial(124.0).at_least(120.0))
            .column(egui_extras::Column::initial(82.0).at_least(72.0))
            .column(egui_extras::Column::initial(92.0).at_least(84.0))
            .column(egui_extras::Column::initial(82.0).at_least(72.0))
            .column(egui_extras::Column::initial(92.0).at_least(84.0))
            .column(egui_extras::Column::initial(56.0).at_least(44.0))
            .column(egui_extras::Column::initial(56.0).at_least(44.0))
            .column(egui_extras::Column::initial(116.0).at_least(108.0))
            .column(egui_extras::Column::initial(110.0).at_least(100.0))
            .column(egui_extras::Column::initial(184.0).at_least(168.0))
            .header(self.text_height + 12.0, |header| self.render_header(header))
            .body(|body| {
                body.rows(self.row_height, self.visible_indices.len(), |row| {
                    self.render_row(row);
                });
            });
    }

    fn render_header(&mut self, mut header: egui_extras::TableRow<'_, '_>) {
        let t = self.t;
        sortable_header(&mut header, self.table_state, SortColumn::Name, t.col_name);
        sortable_header(
            &mut header,
            self.table_state,
            SortColumn::Progress,
            t.col_progress,
        );
        sortable_header(
            &mut header,
            self.table_state,
            SortColumn::UploadSpeed,
            t.col_speed,
        );
        sortable_header(
            &mut header,
            self.table_state,
            SortColumn::Uploaded,
            t.col_uploaded,
        );
        sortable_header(
            &mut header,
            self.table_state,
            SortColumn::DownloadSpeed,
            t.col_dl_speed,
        );
        sortable_header(
            &mut header,
            self.table_state,
            SortColumn::Downloaded,
            t.col_downloaded,
        );
        sortable_header(
            &mut header,
            self.table_state,
            SortColumn::Seeders,
            t.col_seeders,
        );
        sortable_header(
            &mut header,
            self.table_state,
            SortColumn::Leechers,
            t.col_leechers,
        );
        sortable_header(
            &mut header,
            self.table_state,
            SortColumn::LastAnnounce,
            t.col_last_announce,
        );
        sortable_header(
            &mut header,
            self.table_state,
            SortColumn::Health,
            t.col_health,
        );
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
    }

    fn render_row(&mut self, mut row: egui_extras::TableRow<'_, '_>) {
        let row_index = row.index();
        let index = self.visible_indices[row_index];
        let torrent = &mut self.snapshot.torrents[index];
        row.col(|ui| name_cell(ui, row_index, torrent));
        row.col(|ui| progress_cell(ui, row_index, torrent));
        row.col(|ui| {
            speed_cell(
                ui,
                row_index,
                "upload_speed",
                torrent.current_speed_bps,
                theme::text_primary(),
            );
        });
        row.col(|ui| {
            bytes_cell(
                ui,
                row_index,
                "uploaded",
                torrent.uploaded_bytes,
                theme::text_primary(),
            );
        });
        row.col(|ui| {
            speed_cell(
                ui,
                row_index,
                "download_speed",
                torrent.current_download_speed_bps,
                theme::text_secondary(),
            );
        });
        row.col(|ui| {
            bytes_cell(
                ui,
                row_index,
                "downloaded",
                torrent.downloaded_bytes,
                theme::text_secondary(),
            );
        });
        row.col(|ui| optional_count_cell(ui, row_index, "seeders", torrent.last_known_seeders));
        row.col(|ui| optional_count_cell(ui, row_index, "leechers", torrent.last_known_leechers));
        row.col(|ui| announce_meta_cell(ui, row_index, torrent, self.t));
        row.col(|ui| health_cell(ui, row_index, torrent, self.t));
        row.col(|ui| {
            actions_cell(
                ui,
                row_index,
                torrent,
                self.pending_delete,
                self.cmd_tx,
                self.t,
            );
        });
    }
}

fn name_cell(ui: &mut egui::Ui, row_index: usize, torrent: &TorrentStatus) {
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
}

fn progress_cell(ui: &mut egui::Ui, row_index: usize, torrent: &TorrentStatus) {
    cell_scope(ui, row_index, "progress", |ui| {
        let progress = progress_fraction(torrent);
        let tone = if progress >= 1.0 || torrent.initial_completed {
            theme::Tone::Success
        } else {
            theme::Tone::Accent
        };
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
}

fn speed_cell(
    ui: &mut egui::Ui,
    row_index: usize,
    key: &'static str,
    bps: u64,
    color: egui::Color32,
) {
    cell_scope(ui, row_index, key, |ui| {
        ui.label(egui::RichText::new(format_speed(bps)).color(color));
    });
}

fn bytes_cell(
    ui: &mut egui::Ui,
    row_index: usize,
    key: &'static str,
    bytes: u64,
    color: egui::Color32,
) {
    cell_scope(ui, row_index, key, |ui| {
        ui.label(egui::RichText::new(format_bytes(bytes)).color(color));
    });
}

fn optional_count_cell(ui: &mut egui::Ui, row_index: usize, key: &'static str, value: Option<u32>) {
    cell_scope(ui, row_index, key, |ui| {
        ui.label(egui::RichText::new(opt_u32(value)).color(theme::text_primary()));
    });
}

fn announce_meta_cell(ui: &mut egui::Ui, row_index: usize, torrent: &TorrentStatus, t: &Tr) {
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
}

fn actions_cell(
    ui: &mut egui::Ui,
    row_index: usize,
    torrent: &mut TorrentStatus,
    pending_delete: &mut Option<DeleteConfirmation>,
    cmd_tx: &mpsc::Sender<EngineCommand>,
    t: &Tr,
) {
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
                if let Err(error) = cmd_tx.try_send(EngineCommand::SetTorrentInitialCompleted {
                    info_hash: torrent.info_hash.clone(),
                    completed: torrent.initial_completed,
                }) {
                    warn!(%error, "failed to enqueue torrent completion toggle");
                }
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
                    egui::Button::new(egui::RichText::new(button_label).small().strong().color(fg))
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
        // Badge id is anchored to the row's positional slot key, matching the
        // documented egui id-stability convention for table-row widgets.
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

fn cell_scope<R>(
    ui: &mut egui::Ui,
    row_index: usize,
    key: &'static str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    // Each cell's id is anchored on the row's positional `row_index` plus a
    // static per-column key. Using `row_index` (the visual slot, not the
    // torrent identity) keeps widget ids stable across multi-pass layout — the
    // same screen rect always carries the same id chain regardless of how a
    // sibling panel/scrollbar reshapes the central area between passes. This
    // mirrors `.trellis/spec/backend/quality-guidelines.md`'s rule for row /
    // slot-based layouts.
    ui.push_id((row_index, key), add_contents).inner
}
