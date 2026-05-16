#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    #[default]
    Chinese,
    English,
}

impl Language {
    pub fn label(self) -> &'static str {
        match self {
            Self::Chinese => "中文",
            Self::English => "EN",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            Self::Chinese => Self::English,
            Self::English => Self::Chinese,
        }
    }
}

pub struct Tr {
    // Toolbar
    pub stop: &'static str,
    pub start: &'static str,
    pub hide_config: &'static str,
    pub config: &'static str,
    pub add_torrent: &'static str,
    pub announce_all_now: &'static str,

    // Status bar
    pub client: &'static str,
    pub torrents: &'static str,
    pub running: &'static str,
    pub stopped: &'static str,
    pub uptime: &'static str,

    // Speed chart
    pub waiting_for_speed_data: &'static str,
    pub upload_kbs: &'static str,
    pub time_s: &'static str,

    // Torrent table
    pub no_torrents: &'static str,
    pub col_name: &'static str,
    pub col_hash: &'static str,
    pub col_speed: &'static str,
    pub col_uploaded: &'static str,
    pub col_dl_speed: &'static str,
    pub col_downloaded: &'static str,
    pub col_progress: &'static str,
    pub col_seeders: &'static str,
    pub col_leechers: &'static str,
    pub col_status: &'static str,
    pub col_actions: &'static str,
    pub mark_completed_tooltip: &'static str,

    // Log panel
    pub log: &'static str,
    pub auto_scroll: &'static str,
    pub entries: &'static str,

    // Config panel
    pub configuration: &'static str,
    pub min_upload_rate: &'static str,
    pub max_upload_rate: &'static str,
    pub min_download_rate: &'static str,
    pub max_download_rate: &'static str,
    pub simultaneous_seed: &'static str,
    pub upload_ratio_target: &'static str,
    pub client_label: &'static str,
    pub keep_zero_leecher: &'static str,
    pub proxy_optional: &'static str,
    pub proxy_host: &'static str,
    pub proxy_port: &'static str,
    pub tip_ratio: &'static str,
    pub save_and_restart: &'static str,

    // Delete dialog
    pub confirm_delete: &'static str,
    pub delete_prompt: &'static str,
    pub delete_hint: &'static str,
    pub delete: &'static str,
    pub cancel: &'static str,
}

pub fn tr(lang: Language) -> &'static Tr {
    match lang {
        Language::Chinese => &ZH,
        Language::English => &EN,
    }
}

static ZH: Tr = Tr {
    stop: "停止",
    start: "启动",
    hide_config: "隐藏配置",
    config: "配置",
    add_torrent: "添加种子",
    announce_all_now: "立即上报全部",
    client: "客户端",
    torrents: "种子数",
    running: "运行中",
    stopped: "已停止",
    uptime: "运行时间",
    waiting_for_speed_data: "等待速度数据...",
    upload_kbs: "上传 (KB/s)",
    time_s: "时间 (秒)",
    no_torrents: "暂无种子 — 请将 .torrent 文件添加到 torrents/ 目录",
    col_name: "名称",
    col_hash: "哈希",
    col_speed: "▲ 速度",
    col_uploaded: "已上传",
    col_dl_speed: "▼ 速度",
    col_downloaded: "已下载",
    col_progress: "进度",
    col_seeders: "做种",
    col_leechers: "下载",
    col_status: "状态",
    col_actions: "操作",
    mark_completed_tooltip: "标记为已完成（首次 announce 即报满）",
    log: "日志",
    auto_scroll: "自动滚动",
    entries: "条",
    configuration: "配置",
    min_upload_rate: "最小上传速率 (kB/s):",
    max_upload_rate: "最大上传速率 (kB/s):",
    min_download_rate: "最小下载速率 (kB/s):",
    max_download_rate: "最大下载速率 (kB/s):",
    simultaneous_seed: "同时做种数:",
    upload_ratio_target: "上传比率目标:",
    client_label: "客户端:",
    keep_zero_leecher: "保留零下载者种子:",
    proxy_optional: "代理 (可选)",
    proxy_host: "代理主机:",
    proxy_port: "代理端口:",
    tip_ratio: "提示: 比率目标 -1.0 = 永久做种",
    save_and_restart: "保存并重启",
    confirm_delete: "确认删除",
    delete_prompt: "删除种子",
    delete_hint: "文件将被移动到归档目录。",
    delete: "删除",
    cancel: "取消",
};

static EN: Tr = Tr {
    stop: "Stop",
    start: "Start",
    hide_config: "Hide Config",
    config: "Config",
    add_torrent: "Add Torrent",
    announce_all_now: "Announce All Now",
    client: "Client",
    torrents: "Torrents",
    running: "Running",
    stopped: "Stopped",
    uptime: "Uptime",
    waiting_for_speed_data: "Waiting for speed data...",
    upload_kbs: "Upload (KB/s)",
    time_s: "Time (s)",
    no_torrents: "No torrents loaded — add .torrent files to your torrents/ folder",
    col_name: "Name",
    col_hash: "Hash",
    col_speed: "Up Speed",
    col_uploaded: "Uploaded",
    col_dl_speed: "DL Speed",
    col_downloaded: "Downloaded",
    col_progress: "Progress",
    col_seeders: "Seeders",
    col_leechers: "Leechers",
    col_status: "Status",
    col_actions: "Actions",
    mark_completed_tooltip: "Mark as completed (first announce reports as full)",
    log: "Log",
    auto_scroll: "Auto-scroll",
    entries: "entries",
    configuration: "Configuration",
    min_upload_rate: "Min Upload Rate (kB/s):",
    max_upload_rate: "Max Upload Rate (kB/s):",
    min_download_rate: "Min Download Rate (kB/s):",
    max_download_rate: "Max Download Rate (kB/s):",
    simultaneous_seed: "Simultaneous Seed:",
    upload_ratio_target: "Upload Ratio Target:",
    client_label: "Client:",
    keep_zero_leecher: "Keep zero-leecher torrents:",
    proxy_optional: "Proxy (optional)",
    proxy_host: "Proxy Host:",
    proxy_port: "Proxy Port:",
    tip_ratio: "Tip: -1.0 ratio target = seed forever",
    save_and_restart: "Save & Restart",
    confirm_delete: "Confirm Delete",
    delete_prompt: "Delete torrent",
    delete_hint: "The file will be moved to the archive folder.",
    delete: "Delete",
    cancel: "Cancel",
};
