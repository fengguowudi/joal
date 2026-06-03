use joal_core::config::{AppConfiguration, ConfigError, UPLOAD_RATIO_TARGET_DISABLED};
use joal_core::snapshot::EngineSnapshot;

/// Editable config fields mirroring AppConfiguration.
pub(super) struct ConfigEditState {
    pub(super) min_upload_rate: String,
    pub(super) max_upload_rate: String,
    pub(super) min_download_rate: String,
    pub(super) max_download_rate: String,
    pub(super) simultaneous_seed: String,
    pub(super) upload_ratio_target: String,
    pub(super) selected_client: String,
    pub(super) keep_torrent_with_zero_leechers: bool,
    pub(super) proxy_host: String,
    pub(super) proxy_port: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConfigField {
    MinUploadRate,
    MaxUploadRate,
    MinDownloadRate,
    MaxDownloadRate,
    SimultaneousSeed,
    UploadRatioTarget,
}

impl ConfigField {
    fn label(self, t: &super::i18n::Tr) -> &str {
        match self {
            Self::MinUploadRate => t.min_upload_rate,
            Self::MaxUploadRate => t.max_upload_rate,
            Self::MinDownloadRate => t.min_download_rate,
            Self::MaxDownloadRate => t.max_download_rate,
            Self::SimultaneousSeed => t.simultaneous_seed,
            Self::UploadRatioTarget => t.upload_ratio_target,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ConfigValidationIssue {
    InvalidNumber(ConfigField),
    InvalidPort,
    ClientRequired,
    ClientUnavailable,
    ProxyPairRequired,
    UploadRateRange,
    DownloadRateRange,
    SimultaneousSeedTooLow,
    UploadRatioTargetInvalid,
    Unexpected(String),
}

impl ConfigValidationIssue {
    pub(super) fn message(&self, t: &super::i18n::Tr) -> String {
        match self {
            Self::InvalidNumber(field) => format!("{} {}", field.label(t), t.config_invalid_number),
            Self::InvalidPort => format!("{} {}", t.proxy_port, t.config_invalid_port),
            Self::ClientRequired => t.config_client_required.to_owned(),
            Self::ClientUnavailable => t.config_client_unavailable.to_owned(),
            Self::ProxyPairRequired => t.config_proxy_pair_required.to_owned(),
            Self::UploadRateRange => t.config_upload_rate_range.to_owned(),
            Self::DownloadRateRange => t.config_download_rate_range.to_owned(),
            Self::SimultaneousSeedTooLow => t.config_simultaneous_seed_positive.to_owned(),
            Self::UploadRatioTargetInvalid => t.config_upload_ratio_invalid.to_owned(),
            Self::Unexpected(message) => message.clone(),
        }
    }

    fn from_config_error(error: ConfigError) -> Self {
        match error {
            ConfigError::Invalid(
                "maxUploadRate must be greater than or equal to minUploadRate",
            ) => Self::UploadRateRange,
            ConfigError::Invalid(
                "maxDownloadRate must be greater than or equal to minDownloadRate",
            ) => Self::DownloadRateRange,
            ConfigError::Invalid("simultaneousSeed must be greater than 0") => {
                Self::SimultaneousSeedTooLow
            }
            ConfigError::Invalid("client is required, no file name given") => Self::ClientRequired,
            ConfigError::Invalid("uploadRatioTarget must be greater than 0 (or equal to -1)") => {
                Self::UploadRatioTargetInvalid
            }
            other => Self::Unexpected(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConfigNotice {
    SavedAndRestarted,
}

impl ConfigNotice {
    pub(super) fn message(self, t: &super::i18n::Tr) -> &str {
        match self {
            Self::SavedAndRestarted => t.config_saved_restarted,
        }
    }
}

struct ParsedNumericConfig {
    min_upload_rate: u64,
    max_upload_rate: u64,
    min_download_rate: u64,
    max_download_rate: u64,
    simultaneous_seed: u32,
    upload_ratio_target: f32,
}

struct ParsedProxyConfig {
    host: Option<String>,
    port: Option<u16>,
}

impl ConfigEditState {
    pub(super) fn from_snapshot(
        snapshot: &EngineSnapshot,
        config: Option<&AppConfiguration>,
    ) -> Self {
        if let Some(cfg) = config {
            Self {
                min_upload_rate: cfg.min_upload_rate.to_string(),
                max_upload_rate: cfg.max_upload_rate.to_string(),
                min_download_rate: cfg.min_download_rate.to_string(),
                max_download_rate: cfg.max_download_rate.to_string(),
                simultaneous_seed: cfg.simultaneous_seed.to_string(),
                upload_ratio_target: format!("{:.1}", cfg.upload_ratio_target),
                selected_client: cfg.client.clone(),
                keep_torrent_with_zero_leechers: cfg.keep_torrent_with_zero_leechers,
                proxy_host: cfg.proxy_host.clone().unwrap_or_default(),
                proxy_port: cfg.proxy_port.map_or_else(String::new, |p| p.to_string()),
            }
        } else {
            Self {
                min_upload_rate: "30".to_owned(),
                max_upload_rate: "170".to_owned(),
                min_download_rate: "0".to_owned(),
                max_download_rate: "0".to_owned(),
                simultaneous_seed: "5".to_owned(),
                upload_ratio_target: "-1.0".to_owned(),
                selected_client: snapshot.active_client_filename.clone(),
                keep_torrent_with_zero_leechers: true,
                proxy_host: String::new(),
                proxy_port: String::new(),
            }
        }
    }

    pub(super) fn validated_config(
        &self,
        available_clients: &[String],
    ) -> Result<AppConfiguration, Vec<ConfigValidationIssue>> {
        let mut errors = Vec::new();

        let selected_client = self.validate_client_selection(available_clients, &mut errors);
        let proxy = self.validate_proxy_settings(&mut errors);
        let Some(numbers) = self.parse_numeric_config(&mut errors) else {
            return Err(errors);
        };

        validate_numeric_ranges(&numbers, &mut errors);

        let config = AppConfiguration {
            min_upload_rate: numbers.min_upload_rate,
            max_upload_rate: numbers.max_upload_rate,
            min_download_rate: numbers.min_download_rate,
            max_download_rate: numbers.max_download_rate,
            simultaneous_seed: numbers.simultaneous_seed,
            client: selected_client,
            keep_torrent_with_zero_leechers: self.keep_torrent_with_zero_leechers,
            upload_ratio_target: numbers.upload_ratio_target,
            proxy_host: proxy.host,
            proxy_port: proxy.port,
        };

        if let Err(error) = config.validate() {
            push_config_error(&mut errors, ConfigValidationIssue::from_config_error(error));
        }

        if errors.is_empty() {
            Ok(config)
        } else {
            Err(errors)
        }
    }

    fn validate_client_selection(
        &self,
        available_clients: &[String],
        errors: &mut Vec<ConfigValidationIssue>,
    ) -> String {
        let selected_client = self.selected_client.trim().to_owned();
        if selected_client.is_empty() {
            errors.push(ConfigValidationIssue::ClientRequired);
        } else if !available_clients
            .iter()
            .any(|client| client == &selected_client)
        {
            errors.push(ConfigValidationIssue::ClientUnavailable);
        }
        selected_client
    }

    fn validate_proxy_settings(
        &self,
        errors: &mut Vec<ConfigValidationIssue>,
    ) -> ParsedProxyConfig {
        let proxy_host = self.proxy_host.trim().to_owned();
        let proxy_port_text = self.proxy_port.trim();
        let has_proxy_host = !proxy_host.is_empty();
        let has_proxy_port = !proxy_port_text.is_empty();
        if has_proxy_host != has_proxy_port {
            errors.push(ConfigValidationIssue::ProxyPairRequired);
        }

        let port = if has_proxy_port {
            match proxy_port_text.parse::<u16>() {
                Ok(port) if port > 0 => Some(port),
                _ => {
                    errors.push(ConfigValidationIssue::InvalidPort);
                    None
                }
            }
        } else {
            None
        };

        ParsedProxyConfig {
            host: has_proxy_host.then_some(proxy_host),
            port,
        }
    }

    fn parse_numeric_config(
        &self,
        errors: &mut Vec<ConfigValidationIssue>,
    ) -> Option<ParsedNumericConfig> {
        let min_upload_rate =
            parse_config_value::<u64>(&self.min_upload_rate, ConfigField::MinUploadRate, errors);
        let max_upload_rate =
            parse_config_value::<u64>(&self.max_upload_rate, ConfigField::MaxUploadRate, errors);
        let min_download_rate = parse_config_value::<u64>(
            &self.min_download_rate,
            ConfigField::MinDownloadRate,
            errors,
        );
        let max_download_rate = parse_config_value::<u64>(
            &self.max_download_rate,
            ConfigField::MaxDownloadRate,
            errors,
        );
        let simultaneous_seed = parse_config_value::<u32>(
            &self.simultaneous_seed,
            ConfigField::SimultaneousSeed,
            errors,
        );
        let upload_ratio_target = parse_config_value::<f32>(
            &self.upload_ratio_target,
            ConfigField::UploadRatioTarget,
            errors,
        );

        let (
            Some(min_upload_rate),
            Some(max_upload_rate),
            Some(min_download_rate),
            Some(max_download_rate),
            Some(simultaneous_seed),
            Some(upload_ratio_target),
        ) = (
            min_upload_rate,
            max_upload_rate,
            min_download_rate,
            max_download_rate,
            simultaneous_seed,
            upload_ratio_target,
        )
        else {
            return None;
        };

        Some(ParsedNumericConfig {
            min_upload_rate,
            max_upload_rate,
            min_download_rate,
            max_download_rate,
            simultaneous_seed,
            upload_ratio_target,
        })
    }
}

fn validate_numeric_ranges(numbers: &ParsedNumericConfig, errors: &mut Vec<ConfigValidationIssue>) {
    if numbers.max_upload_rate < numbers.min_upload_rate {
        push_config_error(errors, ConfigValidationIssue::UploadRateRange);
    }
    if numbers.max_download_rate < numbers.min_download_rate {
        push_config_error(errors, ConfigValidationIssue::DownloadRateRange);
    }
    if numbers.simultaneous_seed < 1 {
        push_config_error(errors, ConfigValidationIssue::SimultaneousSeedTooLow);
    }
    if numbers.upload_ratio_target < 0.0
        && numbers.upload_ratio_target != UPLOAD_RATIO_TARGET_DISABLED
    {
        push_config_error(errors, ConfigValidationIssue::UploadRatioTargetInvalid);
    }
}

fn parse_config_value<T>(
    value: &str,
    field: ConfigField,
    errors: &mut Vec<ConfigValidationIssue>,
) -> Option<T>
where
    T: std::str::FromStr,
{
    if let Ok(parsed) = value.trim().parse::<T>() {
        Some(parsed)
    } else {
        errors.push(ConfigValidationIssue::InvalidNumber(field));
        None
    }
}

fn push_config_error(errors: &mut Vec<ConfigValidationIssue>, issue: ConfigValidationIssue) {
    if !errors.contains(&issue) {
        errors.push(issue);
    }
}
