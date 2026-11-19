use std::cmp::Ordering;

use joal_core::snapshot::{EngineSnapshot, TorrentStatus};
use joal_core::torrent::InfoHash;

use crate::engine::EnginePhase;

use super::format::{
    format_bytes, format_speed, interval_text, last_announce_text, opt_u32, progress_fraction,
    progress_text, short_hash,
};
use super::health::Health;
use super::{i18n::Tr, theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(super) enum SortColumn {
    #[default]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SortDirection {
    #[default]
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

#[derive(Debug)]
struct SearchKey {
    name_lower: String,
    hash_hex: String,
}

pub(super) struct TableState {
    pub search_query: String,
    pub attention_only: bool,
    sort_column: SortColumn,
    sort_direction: SortDirection,
    query_lower: String,
    search_keys: Vec<SearchKey>,
    membership: Vec<InfoHash>,
    visible: Vec<usize>,
    attention_count: usize,
    #[cfg(test)]
    search_key_rebuilds: usize,
}

impl Default for TableState {
    fn default() -> Self {
        Self {
            search_query: String::new(),
            attention_only: false,
            sort_column: SortColumn::Name,
            sort_direction: SortDirection::Ascending,
            query_lower: String::new(),
            search_keys: Vec::new(),
            membership: Vec::new(),
            visible: Vec::new(),
            attention_count: 0,
            #[cfg(test)]
            search_key_rebuilds: 0,
        }
    }
}

impl TableState {
    pub fn on_snapshot(&mut self, snapshot: &EngineSnapshot) {
        let membership_changed = self.membership.len() != snapshot.torrents.len()
            || self
                .membership
                .iter()
                .zip(snapshot.torrents.iter())
                .any(|(old, torrent)| old != &torrent.info_hash);
        if membership_changed {
            self.membership = snapshot
                .torrents
                .iter()
                .map(|torrent| torrent.info_hash.clone())
                .collect();
            self.search_keys = snapshot
                .torrents
                .iter()
                .map(|torrent| SearchKey {
                    name_lower: torrent.name.to_lowercase(),
                    hash_hex: torrent.info_hash.to_string(),
                })
                .collect();
            #[cfg(test)]
            {
                self.search_key_rebuilds += 1;
            }
        }
        self.recompute_visible(snapshot);
    }

    pub fn set_query(&mut self, query: String, snapshot: &EngineSnapshot) {
        self.query_lower = query.trim().to_lowercase();
        self.search_query = query;
        self.recompute_visible(snapshot);
    }

    pub fn toggle_attention(&mut self, snapshot: &EngineSnapshot) {
        self.attention_only = !self.attention_only;
        self.recompute_visible(snapshot);
    }

    pub fn clear_filters(&mut self, snapshot: &EngineSnapshot) {
        self.search_query.clear();
        self.query_lower.clear();
        self.attention_only = false;
        self.recompute_visible(snapshot);
    }

    pub fn toggle_sort(&mut self, column: SortColumn, snapshot: &EngineSnapshot) {
        if self.sort_column == column {
            self.sort_direction = self.sort_direction.toggled();
        } else {
            self.sort_column = column;
            self.sort_direction = default_sort_direction(column);
        }
        self.recompute_visible(snapshot);
    }

    pub fn visible(&self) -> &[usize] {
        &self.visible
    }

    pub fn attention_count(&self) -> usize {
        self.attention_count
    }

    fn recompute_visible(&mut self, snapshot: &EngineSnapshot) {
        self.attention_count = snapshot
            .torrents
            .iter()
            .filter(|torrent| Health::of(torrent).needs_attention())
            .count();
        self.visible = snapshot
            .torrents
            .iter()
            .enumerate()
            .filter(|(index, torrent)| {
                let key = &self.search_keys[*index];
                (self.query_lower.is_empty()
                    || key.name_lower.contains(&self.query_lower)
                    || key.hash_hex.contains(&self.query_lower))
                    && (!self.attention_only || Health::of(torrent).needs_attention())
            })
            .map(|(index, _)| index)
            .collect();
        let direction = self.sort_direction;
        let column = self.sort_column;
        let keys = &self.search_keys;
        self.visible.sort_by(|left, right| {
            compare_torrents(
                &snapshot.torrents[*left],
                &snapshot.torrents[*right],
                &keys[*left],
                &keys[*right],
                column,
                direction,
            )
        });
    }
}

pub(super) fn toolbar(
    ui: &mut egui::Ui,
    snapshot: &EngineSnapshot,
    state: &mut TableState,
    t: &Tr,
) {
    let focus_shortcut = ui
        .ctx()
        .input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::F));
    ui.horizontal_wrapped(|ui| {
        let mut query = state.search_query.clone();
        let search_response = ui
            .push_id("torrent_table_search", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut query)
                        .id_salt("torrent_table_search")
                        .hint_text(t.search_torrents)
                        .desired_width(240.0),
                )
            })
            .inner;
        if focus_shortcut {
            search_response.request_focus();
        }
        theme::focus_ring(ui, &search_response, theme::CR_INSET);
        if search_response.changed() {
            state.set_query(query, snapshot);
        }
        if !state.search_query.is_empty()
            && theme::quiet_secondary_button(
                ui,
                "clear_torrent_search",
                "×",
                egui::vec2(30.0, 30.0),
            )
            .clicked()
        {
            state.set_query(String::new(), snapshot);
        }

        if theme::tone_button(
            ui,
            "torrent_table_attention_toggle",
            &format!("{} {}", t.attention, state.attention_count()),
            if state.attention_count() > 0 {
                theme::Tone::Warning
            } else {
                theme::Tone::Neutral
            },
            egui::vec2(140.0, 30.0),
            state.attention_only,
        )
        .on_hover_text(t.attention_hint)
        .clicked()
        {
            state.toggle_attention(snapshot);
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} / {}",
                    state.visible().len(),
                    snapshot.torrents.len()
                ))
                .color(theme::text_secondary()),
            );
        });
    });
}

pub(super) enum TableAction {
    Sort(SortColumn),
    ToggleCompleted {
        info_hash: InfoHash,
        completed: bool,
    },
    RequestArchive {
        info_hash: InfoHash,
        name: String,
    },
    AddTorrent,
    ClearFilters,
}

#[derive(Clone, Copy)]
pub(super) struct TableView {
    pub download_enabled: bool,
    pub phase: EnginePhase,
}

pub fn show(
    ui: &mut egui::Ui,
    snapshot: &EngineSnapshot,
    state: &TableState,
    view: TableView,
    t: &Tr,
) -> Option<TableAction> {
    if snapshot.torrents.is_empty() {
        let mut action = None;
        ui.centered_and_justified(|ui| {
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(t.no_torrents).color(theme::text_secondary()));
                let phase_text = match view.phase {
                    EnginePhase::Starting => Some(t.starting),
                    EnginePhase::Stopped => Some(t.stopped),
                    EnginePhase::Failed => Some(t.start_failed),
                    EnginePhase::Running | EnginePhase::Stopping => None,
                };
                if let Some(text) = phase_text {
                    ui.label(
                        egui::RichText::new(text)
                            .small()
                            .color(theme::text_secondary()),
                    );
                }
                ui.add_space(8.0);
                if theme::secondary_button(
                    ui,
                    "empty_state_add_torrent",
                    t.add_torrent,
                    egui::vec2(124.0, 32.0),
                )
                .clicked()
                {
                    action = Some(TableAction::AddTorrent);
                }
                ui.label(
                    egui::RichText::new(t.drop_hint)
                        .small()
                        .color(theme::text_secondary()),
                );
            });
        });
        return action;
    }
    if state.visible().is_empty() {
        let mut action = None;
        ui.centered_and_justified(|ui| {
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(t.no_matching_torrents).color(theme::text_secondary()),
                );
                ui.add_space(8.0);
                if theme::secondary_button(
                    ui,
                    "clear_table_filters",
                    t.clear_filters,
                    egui::vec2(124.0, 32.0),
                )
                .clicked()
                {
                    action = Some(TableAction::ClearFilters);
                }
            });
        });
        return action;
    }

    let text_height = egui::TextStyle::Body.resolve(ui.style()).size;
    let row_height = (text_height * 2.1).max(36.0);
    let available_height = ui.available_height();
    let columns = columns(view.download_enabled);
    let min_table_width = columns.iter().map(|kind| column_spec(*kind).1).sum::<f32>() + 8.0;
    let mut action = None;
    egui::ScrollArea::horizontal()
        .id_salt("torrent_table_hscroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(min_table_width);
            action = TableRender {
                snapshot,
                state,
                columns,
                view,
                t,
                row_height,
                available_height,
            }
            .show(ui);
        });
    action
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColumnKind {
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
    Actions,
}

const SEED_COLUMNS: [ColumnKind; 9] = [
    ColumnKind::Name,
    ColumnKind::Progress,
    ColumnKind::UploadSpeed,
    ColumnKind::Uploaded,
    ColumnKind::Seeders,
    ColumnKind::Leechers,
    ColumnKind::LastAnnounce,
    ColumnKind::Health,
    ColumnKind::Actions,
];
const FULL_COLUMNS: [ColumnKind; 11] = [
    ColumnKind::Name,
    ColumnKind::Progress,
    ColumnKind::UploadSpeed,
    ColumnKind::Uploaded,
    ColumnKind::DownloadSpeed,
    ColumnKind::Downloaded,
    ColumnKind::Seeders,
    ColumnKind::Leechers,
    ColumnKind::LastAnnounce,
    ColumnKind::Health,
    ColumnKind::Actions,
];

fn columns(download_enabled: bool) -> &'static [ColumnKind] {
    if download_enabled {
        &FULL_COLUMNS
    } else {
        &SEED_COLUMNS
    }
}

fn column_spec(kind: ColumnKind) -> (f32, f32, Option<f32>) {
    match kind {
        ColumnKind::Name => (240.0, 160.0, Some(360.0)),
        ColumnKind::Progress => (96.0, 88.0, None),
        ColumnKind::UploadSpeed
        | ColumnKind::Uploaded
        | ColumnKind::DownloadSpeed
        | ColumnKind::Downloaded => (92.0, 84.0, None),
        ColumnKind::Seeders | ColumnKind::Leechers => (56.0, 48.0, None),
        ColumnKind::LastAnnounce => (112.0, 104.0, None),
        ColumnKind::Health => (116.0, 104.0, None),
        ColumnKind::Actions => (176.0, 168.0, None),
    }
}

struct TableRender<'a> {
    snapshot: &'a EngineSnapshot,
    state: &'a TableState,
    columns: &'static [ColumnKind],
    view: TableView,
    t: &'a Tr,
    row_height: f32,
    available_height: f32,
}

impl TableRender<'_> {
    fn show(&mut self, ui: &mut egui::Ui) -> Option<TableAction> {
        let mut action = None;
        let text_height = egui::TextStyle::Body.resolve(ui.style()).size;
        let mut builder = egui_extras::TableBuilder::new(ui)
            .id_salt(if self.view.download_enabled {
                "torrent_table_full"
            } else {
                "torrent_table_seed"
            })
            .striped(true)
            .resizable(true)
            .vscroll(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .min_scrolled_height(self.available_height.max(120.0))
            .max_scroll_height(self.available_height.max(120.0));
        for kind in self.columns {
            let (initial, at_least, at_most) = column_spec(*kind);
            let mut column = egui_extras::Column::initial(initial).at_least(at_least);
            if let Some(at_most) = at_most {
                column = column.at_most(at_most).clip(true);
            }
            builder = builder.column(column);
        }
        builder
            .header(text_height + 12.0, |header| {
                if action.is_none() {
                    action = self.render_header(header);
                }
            })
            .body(|body| {
                body.rows(self.row_height, self.state.visible().len(), |row| {
                    if action.is_none() {
                        action = self.render_row(row);
                    }
                });
            });
        action
    }

    fn render_header(&self, mut header: egui_extras::TableRow<'_, '_>) -> Option<TableAction> {
        let mut action = None;
        for kind in self.columns {
            let sort_column = match kind {
                ColumnKind::Name => Some(SortColumn::Name),
                ColumnKind::Progress => Some(SortColumn::Progress),
                ColumnKind::UploadSpeed => Some(SortColumn::UploadSpeed),
                ColumnKind::Uploaded => Some(SortColumn::Uploaded),
                ColumnKind::DownloadSpeed => Some(SortColumn::DownloadSpeed),
                ColumnKind::Downloaded => Some(SortColumn::Downloaded),
                ColumnKind::Seeders => Some(SortColumn::Seeders),
                ColumnKind::Leechers => Some(SortColumn::Leechers),
                ColumnKind::LastAnnounce => Some(SortColumn::LastAnnounce),
                ColumnKind::Health => Some(SortColumn::Health),
                ColumnKind::Actions => None,
            };
            let label = match kind {
                ColumnKind::Name => self.t.col_name,
                ColumnKind::Progress => self.t.col_progress,
                ColumnKind::UploadSpeed => self.t.col_speed,
                ColumnKind::Uploaded => self.t.col_uploaded,
                ColumnKind::DownloadSpeed => self.t.col_dl_speed,
                ColumnKind::Downloaded => self.t.col_downloaded,
                ColumnKind::Seeders => self.t.col_seeders,
                ColumnKind::Leechers => self.t.col_leechers,
                ColumnKind::LastAnnounce => self.t.col_last_announce,
                ColumnKind::Health => self.t.col_health,
                ColumnKind::Actions => self.t.col_actions,
            };
            if let Some(column) = sort_column {
                if sortable_header(&mut header, self.state, column, label) {
                    action = Some(TableAction::Sort(column));
                }
            } else {
                header.col(|ui| {
                    ui.label(
                        egui::RichText::new(label)
                            .small()
                            .color(theme::text_secondary()),
                    );
                });
            }
        }
        action
    }

    fn render_row(&self, mut row: egui_extras::TableRow<'_, '_>) -> Option<TableAction> {
        let row_index = row.index();
        let index = self.state.visible()[row_index];
        let torrent = &self.snapshot.torrents[index];
        let mut action = None;
        for kind in self.columns {
            match kind {
                ColumnKind::Name => {
                    row.col(|ui| name_cell(ui, row_index, torrent));
                }
                ColumnKind::Progress => {
                    row.col(|ui| progress_cell(ui, row_index, torrent));
                }
                ColumnKind::UploadSpeed => {
                    row.col(|ui| {
                        value_cell(
                            ui,
                            row_index,
                            "upload_speed",
                            format_speed(torrent.current_speed_bps),
                            theme::text_primary(),
                        );
                    });
                }
                ColumnKind::Uploaded => {
                    row.col(|ui| {
                        value_cell(
                            ui,
                            row_index,
                            "uploaded",
                            format_bytes(torrent.uploaded_bytes),
                            theme::text_primary(),
                        );
                    });
                }
                ColumnKind::DownloadSpeed => {
                    row.col(|ui| {
                        value_cell(
                            ui,
                            row_index,
                            "download_speed",
                            format_speed(torrent.current_download_speed_bps),
                            theme::text_secondary(),
                        );
                    });
                }
                ColumnKind::Downloaded => {
                    row.col(|ui| {
                        value_cell(
                            ui,
                            row_index,
                            "downloaded",
                            format_bytes(torrent.downloaded_bytes),
                            theme::text_secondary(),
                        );
                    });
                }
                ColumnKind::Seeders => {
                    row.col(|ui| {
                        optional_count_cell(ui, row_index, "seeders", torrent.last_known_seeders);
                    });
                }
                ColumnKind::Leechers => {
                    row.col(|ui| {
                        optional_count_cell(ui, row_index, "leechers", torrent.last_known_leechers);
                    });
                }
                ColumnKind::LastAnnounce => {
                    row.col(|ui| announce_meta_cell(ui, row_index, torrent, self.t));
                }
                ColumnKind::Health => {
                    row.col(|ui| health_cell(ui, row_index, torrent, self.t));
                }
                ColumnKind::Actions => {
                    row.col(|ui| {
                        if action.is_none() {
                            action = actions_cell(ui, row_index, torrent, self.t);
                        }
                    });
                }
            }
        }
        action
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
            .on_hover_ui(|ui| {
                ui.label(&torrent.name);
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(short_hash(torrent))
                        .monospace()
                        .small()
                        .color(theme::text_tertiary()),
                )
                .truncate(),
            )
            .on_hover_ui(|ui| {
                ui.label(torrent.info_hash.to_string());
            });
        });
    });
}

fn progress_cell(ui: &mut egui::Ui, row_index: usize, torrent: &TorrentStatus) {
    cell_scope(ui, row_index, "progress", |ui| {
        ui.vertical(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(progress_text(torrent)).color(theme::text_primary()));
            });
            let progress = progress_fraction(torrent) as f32;
            let width = ui.available_width().max(1.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 4.0), egui::Sense::hover());
            let radius = egui::CornerRadius::same(2);
            ui.painter()
                .rect_filled(rect, radius, egui::Color32::from_rgb(226, 232, 240));
            let fill = if progress >= 1.0 || torrent.initial_completed {
                theme::tone_colors(theme::Tone::Success).fg
            } else {
                theme::primary_color()
            };
            let mut filled = rect;
            filled.max.x = rect.min.x + rect.width() * progress;
            ui.painter().rect_filled(filled, radius, fill);
        });
    });
}

fn value_cell(
    ui: &mut egui::Ui,
    row_index: usize,
    key: &'static str,
    text: String,
    color: egui::Color32,
) {
    cell_scope(ui, row_index, key, |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(text).color(color));
        });
    });
}

fn optional_count_cell(ui: &mut egui::Ui, row_index: usize, key: &'static str, value: Option<u32>) {
    value_cell(ui, row_index, key, opt_u32(value), theme::text_primary());
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
    torrent: &TorrentStatus,
    t: &Tr,
) -> Option<TableAction> {
    cell_scope(ui, row_index, "actions", |ui| {
        let mut action = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let mark_label = if torrent.initial_completed {
                t.action_marked_complete
            } else {
                t.action_mark_complete
            };
            let response = if torrent.initial_completed {
                theme::tone_button(
                    ui,
                    "mark_completed",
                    mark_label,
                    theme::Tone::Success,
                    egui::vec2(88.0, 24.0),
                    true,
                )
            } else {
                theme::quiet_secondary_button(
                    ui,
                    "mark_completed",
                    mark_label,
                    egui::vec2(88.0, 24.0),
                )
            }
            .on_hover_text(t.mark_completed_tooltip);
            if response.clicked() {
                action = Some(TableAction::ToggleCompleted {
                    info_hash: torrent.info_hash.clone(),
                    completed: !torrent.initial_completed,
                });
            }
            if theme::quiet_secondary_button(
                ui,
                "archive_torrent",
                t.action_archive,
                egui::vec2(68.0, 24.0),
            )
            .clicked()
            {
                action = Some(TableAction::RequestArchive {
                    info_hash: torrent.info_hash.clone(),
                    name: torrent.name.clone(),
                });
            }
        });
        action
    })
    .inner
}

fn compare_torrents(
    left: &TorrentStatus,
    right: &TorrentStatus,
    left_key: &SearchKey,
    right_key: &SearchKey,
    column: SortColumn,
    direction: SortDirection,
) -> Ordering {
    let ordering = match column {
        SortColumn::Name => left_key.name_lower.cmp(&right_key.name_lower),
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
        SortColumn::Health => (
            Health::of(left),
            left.consecutive_fails,
            left.last_known_leechers.unwrap_or_default(),
        )
            .cmp(&(
                Health::of(right),
                right.consecutive_fails,
                right.last_known_leechers.unwrap_or_default(),
            )),
    };
    match direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
    }
    .then_with(|| left_key.name_lower.cmp(&right_key.name_lower))
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

fn sortable_header(
    header: &mut egui_extras::TableRow<'_, '_>,
    state: &TableState,
    column: SortColumn,
    label: &str,
) -> bool {
    let mut clicked = false;
    header.col(|ui| {
        let active = state.sort_column == column;
        let min_width = (ui.available_width() - 4.0).max(48.0);
        let button_label = active.then(|| format!("{label}  "));
        let response = theme::quiet_secondary_button(
            ui,
            ("torrent_table_sort", column),
            button_label.as_deref().unwrap_or(label),
            egui::vec2(min_width, 22.0),
        );
        if active {
            let center = response.rect.right_center() - egui::vec2(10.0, 0.0);
            let points = match state.sort_direction {
                SortDirection::Ascending => vec![
                    center + egui::vec2(-4.0, 3.0),
                    center + egui::vec2(4.0, 3.0),
                    center + egui::vec2(0.0, -3.0),
                ],
                SortDirection::Descending => vec![
                    center + egui::vec2(-4.0, -3.0),
                    center + egui::vec2(4.0, -3.0),
                    center + egui::vec2(0.0, 3.0),
                ],
            };
            ui.painter().add(egui::Shape::convex_polygon(
                points,
                theme::primary_color(),
                egui::Stroke::NONE,
            ));
        }
        clicked = response.clicked();
    });
    clicked
}

fn health_cell(ui: &mut egui::Ui, row_index: usize, torrent: &TorrentStatus, t: &Tr) {
    let health = Health::of(torrent);
    cell_scope(ui, row_index, "health", |ui| {
        let response = ui
            .vertical(|ui| {
                theme::badge(
                    ui,
                    (row_index, "health_badge"),
                    health.label(t),
                    health.tone(),
                );
            })
            .response;
        response.on_hover_ui(|ui| {
            let detail = health_detail(torrent, t);
            ui.label(detail);
        });
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
) -> egui::InnerResponse<R> {
    ui.push_id((row_index, key), add_contents)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    fn torrent(byte: u8, name: &str, speed: u64, announced: bool) -> TorrentStatus {
        TorrentStatus {
            info_hash: InfoHash::from_bytes([byte; 20]),
            name: name.to_owned(),
            total_size: 100,
            uploaded_bytes: 0,
            downloaded_bytes: 0,
            left_bytes: 100,
            current_speed_bps: speed,
            current_download_speed_bps: 0,
            initial_completed: false,
            last_known_interval: None,
            last_known_seeders: Some(1),
            last_known_leechers: Some(1),
            consecutive_fails: 0,
            last_announced_at: announced.then(Instant::now),
        }
    }

    fn snapshot(torrents: Vec<TorrentStatus>) -> EngineSnapshot {
        EngineSnapshot {
            torrents: torrents.into(),
            ..EngineSnapshot::default()
        }
    }

    #[test]
    fn filters_sorts_and_reuses_search_keys() {
        let first = torrent(1, "Alpha", 10, true);
        let second = torrent(2, "Beta", 30, true);
        let third = torrent(3, "Pending", 20, false);
        let snapshot = snapshot(vec![first, second, third]);
        let mut state = TableState::default();
        state.on_snapshot(&snapshot);
        assert_eq!(state.visible(), &[0, 1, 2]);
        assert_eq!(state.attention_count(), 1);

        state.set_query("beta".into(), &snapshot);
        assert_eq!(state.visible(), &[1]);
        state.clear_filters(&snapshot);
        state.toggle_attention(&snapshot);
        assert_eq!(state.visible(), &[2]);
        state.toggle_attention(&snapshot);
        state.toggle_sort(SortColumn::UploadSpeed, &snapshot);
        assert_eq!(state.visible(), &[1, 2, 0]);

        let rebuilds = state.search_key_rebuilds;
        let mut faster = snapshot;
        faster.torrents = faster
            .torrents
            .iter()
            .cloned()
            .map(|mut torrent| {
                torrent.current_speed_bps += 1;
                torrent
            })
            .collect::<Vec<_>>()
            .into();
        state.on_snapshot(&faster);
        assert_eq!(state.search_key_rebuilds, rebuilds);
    }
}
