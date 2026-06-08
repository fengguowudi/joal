//! Runtime emulated BitTorrent client: query + header template engine.
//!
//! Port of Java `org.araymond.joal.core.client.emulated.BitTorrentClient`.
//! The Java class owns a `Set<Map.Entry<String,String>>` of headers and a
//! `createRequestQuery(...)` method that fills in placeholders from the
//! torrent/seed state at announce time. This module does the same, staying
//! byte-compatible with the Java output.
//!
//! JOAL on the Rust side does not embed a JVM, so the three environment-
//! dependent header placeholders are sourced like this:
//!
//! | Placeholder | Java source                            | Rust source                 |
//! |-------------|----------------------------------------|-----------------------------|
//! | `{java}`    | `System.getProperty("java.version")`   | constant [`DEFAULT_JAVA_VERSION`] |
//! | `{os}`      | `System.getProperty("os.name")`        | [`std::env::consts::OS`]    |
//! | `{locale}`  | `Locale.getDefault().toLanguageTag()`  | [`sys_locale::get_locale`]  |
//!
//! `DEFAULT_JAVA_VERSION` keeps the header template deterministic — without a
//! JVM we have nothing more specific to report, and `.client` templates use
//! this only inside `User-Agent`-style strings where any stable value works.

use std::fmt::Write as _;
use std::net::IpAddr;
use std::sync::OnceLock;

use regex::Regex;

use crate::bandwidth::TorrentSeedStats;
use crate::client::config::{BitTorrentClientConfig, HttpHeader};
use crate::client::error::ClientError;
use crate::client::event::RequestEvent;
use crate::client::generator::{KeyGenerator, NumwantProvider, PeerIdGenerator, UrlEncoder};
use crate::client::runtime::ConnectionHandler;
use crate::torrent::InfoHash;

/// Fallback `{java}` header value used when no JVM is around.
pub const DEFAULT_JAVA_VERSION: &str = "17";

/// Compose announce-query strings + static request headers for a single
/// emulated client profile.
#[derive(Debug, Clone)]
pub struct BitTorrentClient {
    peer_id_generator: PeerIdGenerator,
    key_generator: Option<KeyGenerator>,
    url_encoder: UrlEncoder,
    numwant_provider: NumwantProvider,
    query: String,
    query_template: Vec<QueryPairTemplate>,
    requires_key: bool,
    headers: Vec<(String, String)>,
}

impl BitTorrentClient {
    /// Build a runtime client from a parsed `.client` config.
    ///
    /// Mirrors Java `BitTorrentClientProvider.createClient(...)` plus the
    /// `BitTorrentClient` constructor's Precondition checks. The header
    /// templates are resolved eagerly here: `{java}`, `{os}` and `{locale}`
    /// are baked in at construction time because they are process-wide
    /// constants; the remaining placeholders are resolved per-announce.
    pub fn new(config: BitTorrentClientConfig) -> Result<Self, ClientError> {
        config.validate()?;

        let BitTorrentClientConfig {
            peer_id_generator,
            query,
            key_generator,
            url_encoder,
            request_headers,
            numwant,
            numwant_on_stop,
        } = config;

        if query.trim().is_empty() {
            return Err(ClientError::Integrity(
                "query cannot be null or empty".to_owned(),
            ));
        }

        let numwant_provider = NumwantProvider::new(numwant, numwant_on_stop)?;
        let query = collapse_ampersands(&query);
        let query_template = parse_query_template(&query);
        let requires_key = query.contains("{key}");
        let headers = resolve_request_headers(&request_headers)?;

        Ok(Self {
            peer_id_generator,
            key_generator,
            url_encoder,
            numwant_provider,
            query,
            query_template,
            requires_key,
            headers,
        })
    }

    /// Raw query template with consecutive `&` already collapsed.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Resolved `(name, value)` request headers in declaration order.
    #[must_use]
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// Peer-id for the given `(info_hash, event)` pair, applying the
    /// configured refresh policy. The returned bytes are the raw wire
    /// representation — for high-byte regex patterns (rtorrent / bittorrent
    /// uTorrent clients) some bytes are non-ASCII and must be URL-encoded
    /// before going on the announce URL.
    pub fn peer_id(
        &self,
        info_hash: &InfoHash,
        event: RequestEvent,
    ) -> Result<Vec<u8>, ClientError> {
        self.peer_id_generator.get(info_hash, event)
    }

    /// Returns `None` when the `.client` config has no `keyGenerator`.
    pub fn key(
        &self,
        info_hash: &InfoHash,
        event: RequestEvent,
    ) -> Result<Option<Vec<u8>>, ClientError> {
        match &self.key_generator {
            Some(generator) => generator.get(info_hash, event).map(Some),
            None => Ok(None),
        }
    }

    /// Numwant value for `event` (honors `numwantOnStop` on `Stopped`).
    #[must_use]
    pub fn numwant(&self, event: RequestEvent) -> i32 {
        self.numwant_provider.get(event)
    }

    /// Build the URL-encoded announce query string for a single request.
    ///
    /// Order matches Java's `BitTorrentClient.createRequestQuery(...)`
    /// exactly so that a tracker-visible diff between the Java and Rust
    /// outputs is zero for equivalent inputs.
    pub fn create_request_query(
        &self,
        event: RequestEvent,
        info_hash: &InfoHash,
        stats: &TorrentSeedStats,
        connection: &ConnectionHandler,
    ) -> Result<String, ClientError> {
        let values = QueryValues::new(self, event, info_hash, stats, connection)?;
        let mut out = String::with_capacity(self.query.len() + 32);
        for pair in &self.query_template {
            if !pair.should_emit(&values) {
                continue;
            }
            if !out.is_empty() {
                out.push('&');
            }
            pair.render(&values, &mut out)?;
        }

        if let Some(m) = placeholder_regex().find(&out) {
            return Err(ClientError::Integrity(format!(
                "Placeholder [{}] were not recognized while building announce URL",
                m.as_str()
            )));
        }

        Ok(out)
    }
}

#[derive(Debug, Clone)]
struct QueryPairTemplate {
    segments: Vec<QuerySegment>,
    skip_when_event_none: bool,
    ip_family: IpPairFamily,
}

#[derive(Debug, Clone)]
enum QuerySegment {
    Literal(String),
    Placeholder(QueryPlaceholder),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryPlaceholder {
    InfoHash,
    Uploaded,
    Downloaded,
    Left,
    Port,
    Numwant,
    PeerId,
    Event,
    Key,
    Ip,
    Ipv6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IpPairFamily {
    None,
    V4,
    V6,
}

struct QueryValues {
    info_hash: String,
    uploaded: u64,
    downloaded: u64,
    left: u64,
    port: u16,
    numwant: i32,
    peer_id: String,
    event: RequestEvent,
    key: Option<String>,
    ip: Option<String>,
    ipv6: Option<String>,
}

impl QueryValues {
    fn new(
        client: &BitTorrentClient,
        event: RequestEvent,
        info_hash: &InfoHash,
        stats: &TorrentSeedStats,
        connection: &ConnectionHandler,
    ) -> Result<Self, ClientError> {
        let peer_id_raw = client.peer_id(info_hash, event)?;
        let peer_id = if client.peer_id_generator.config().should_url_encode() {
            client.url_encoder.encode_bytes(&peer_id_raw)?
        } else {
            // `should_url_encode = false` is only safe for ASCII-only peer-id
            // patterns; the bundled clients honor this, but a future config
            // could violate the invariant — error out cleanly rather than
            // silently sending non-ASCII bytes on the wire.
            if !peer_id_raw.is_ascii() {
                return Err(ClientError::Integrity(
                    "peer_id contains non-ASCII bytes but shouldUrlEncode is false".to_owned(),
                ));
            }
            String::from_utf8(peer_id_raw).map_err(|error| {
                ClientError::Integrity(format!(
                    "peer_id bytes were ASCII but not valid UTF-8: {error}"
                ))
            })?
        };

        let key = if client.requires_key {
            let raw = client.key(info_hash, event)?.ok_or_else(|| {
                ClientError::Integrity(
                    "Client request query contains 'key' but BitTorrentClient does not have a key"
                        .to_owned(),
                )
            })?;
            Some(client.url_encoder.encode_bytes(&raw)?)
        } else {
            None
        };

        let (ip, ipv6) = match connection.ip_address() {
            Some(IpAddr::V4(v4)) => (Some(v4.to_string()), None),
            Some(IpAddr::V6(v6)) => (None, Some(client.url_encoder.encode(&v6.to_string())?)),
            None => (None, None),
        };

        Ok(Self {
            info_hash: client.url_encoder.encode_bytes(info_hash.as_bytes())?,
            uploaded: stats.uploaded(),
            downloaded: stats.downloaded(),
            left: stats.left(),
            port: connection.port(),
            numwant: client.numwant(event),
            peer_id,
            event,
            key,
            ip,
            ipv6,
        })
    }
}

impl QueryPairTemplate {
    fn should_emit(&self, values: &QueryValues) -> bool {
        if self.skip_when_event_none && values.event == RequestEvent::None {
            return false;
        }
        match self.ip_family {
            IpPairFamily::None => true,
            IpPairFamily::V4 => values.ip.is_some(),
            IpPairFamily::V6 => values.ipv6.is_some(),
        }
    }

    fn render(&self, values: &QueryValues, out: &mut String) -> Result<(), ClientError> {
        for segment in &self.segments {
            match segment {
                QuerySegment::Literal(literal) => out.push_str(literal),
                QuerySegment::Placeholder(placeholder) => placeholder.render(values, out)?,
            }
        }
        Ok(())
    }
}

impl QueryPlaceholder {
    fn render(self, values: &QueryValues, out: &mut String) -> Result<(), ClientError> {
        match self {
            Self::InfoHash => out.push_str(&values.info_hash),
            Self::Uploaded => {
                let _ = write!(out, "{}", values.uploaded);
            }
            Self::Downloaded => {
                let _ = write!(out, "{}", values.downloaded);
            }
            Self::Left => {
                let _ = write!(out, "{}", values.left);
            }
            Self::Port => {
                let _ = write!(out, "{}", values.port);
            }
            Self::Numwant => {
                let _ = write!(out, "{}", values.numwant);
            }
            Self::PeerId => out.push_str(&values.peer_id),
            Self::Event => out.push_str(values.event.event_name()),
            Self::Key => out.push_str(values.key.as_deref().ok_or_else(|| {
                ClientError::Integrity(
                    "Client request query contains 'key' but BitTorrentClient does not have a key"
                        .to_owned(),
                )
            })?),
            Self::Ip => out.push_str(values.ip.as_deref().unwrap_or("{ip}")),
            Self::Ipv6 => out.push_str(values.ipv6.as_deref().unwrap_or("{ipv6}")),
        }
        Ok(())
    }
}

fn parse_query_template(query: &str) -> Vec<QueryPairTemplate> {
    trim_ampersands(query)
        .split('&')
        .filter(|part| !part.is_empty())
        .map(parse_query_pair_template)
        .collect()
}

fn parse_query_pair_template(pair: &str) -> QueryPairTemplate {
    let mut segments = Vec::new();
    let mut rest = pair;
    while let Some(start) = rest.find('{') {
        if start > 0 {
            segments.push(QuerySegment::Literal(rest[..start].to_owned()));
        }
        let after_start = &rest[start..];
        if let Some(end) = after_start.find('}') {
            let placeholder_text = &after_start[..=end];
            if let Some(placeholder) = known_placeholder(placeholder_text) {
                segments.push(QuerySegment::Placeholder(placeholder));
            } else {
                segments.push(QuerySegment::Literal(placeholder_text.to_owned()));
            }
            rest = &after_start[end + 1..];
        } else {
            segments.push(QuerySegment::Literal(after_start.to_owned()));
            rest = "";
        }
    }
    if !rest.is_empty() {
        segments.push(QuerySegment::Literal(rest.to_owned()));
    }
    let skip_when_event_none = segments
        .iter()
        .any(|segment| matches!(segment, QuerySegment::Placeholder(QueryPlaceholder::Event)));
    let ip_family = if segments
        .iter()
        .any(|segment| matches!(segment, QuerySegment::Placeholder(QueryPlaceholder::Ipv6)))
    {
        IpPairFamily::V6
    } else if segments
        .iter()
        .any(|segment| matches!(segment, QuerySegment::Placeholder(QueryPlaceholder::Ip)))
    {
        IpPairFamily::V4
    } else {
        IpPairFamily::None
    };
    QueryPairTemplate {
        segments,
        skip_when_event_none,
        ip_family,
    }
}

fn known_placeholder(text: &str) -> Option<QueryPlaceholder> {
    match text {
        "{infohash}" => Some(QueryPlaceholder::InfoHash),
        "{uploaded}" => Some(QueryPlaceholder::Uploaded),
        "{downloaded}" => Some(QueryPlaceholder::Downloaded),
        "{left}" => Some(QueryPlaceholder::Left),
        "{port}" => Some(QueryPlaceholder::Port),
        "{numwant}" => Some(QueryPlaceholder::Numwant),
        "{peerid}" => Some(QueryPlaceholder::PeerId),
        "{event}" => Some(QueryPlaceholder::Event),
        "{key}" => Some(QueryPlaceholder::Key),
        "{ip}" => Some(QueryPlaceholder::Ip),
        "{ipv6}" => Some(QueryPlaceholder::Ipv6),
        _ => None,
    }
}

fn replace_literal(haystack: &str, pattern: &Regex, replacement: &str) -> String {
    pattern
        .replace_all(haystack, regex::NoExpand(replacement))
        .into_owned()
}

fn collapse_ampersands(input: &str) -> String {
    ampersand_dupes_regex()
        .replace_all(input, regex::NoExpand("&"))
        .into_owned()
}

fn trim_ampersands(s: &str) -> &str {
    s.trim_matches('&')
}

fn resolve_request_headers(raw: &[HttpHeader]) -> Result<Vec<(String, String)>, ClientError> {
    let java_version = DEFAULT_JAVA_VERSION.to_owned();
    let os_name = std::env::consts::OS.to_owned();
    let locale = sys_locale::get_locale().unwrap_or_else(|| "en-US".to_owned());

    let mut resolved = Vec::with_capacity(raw.len());
    for header in raw {
        let mut value = replace_literal(&header.value, &JAVA_PTRN, &java_version);
        value = replace_literal(&value, &OS_PTRN, &os_name);
        value = replace_literal(&value, &LOCALE_PTRN, &locale);

        if let Some(m) = placeholder_regex().find(&value) {
            return Err(ClientError::Integrity(format!(
                "Placeholder [{}] were not recognized while building client Headers",
                m.as_str()
            )));
        }
        resolved.push((header.name.clone(), value));
    }
    Ok(resolved)
}

// ---------------------------------------------------------------------------
//  Regex cache — compiled once per process, same patterns as Java
// ---------------------------------------------------------------------------

macro_rules! cached_regex {
    ($name:ident, $pat:expr) => {
        static $name: LazyRegex = LazyRegex::new($pat);
    };
}

struct LazyRegex {
    pattern: &'static str,
    cell: OnceLock<Regex>,
}

impl LazyRegex {
    const fn new(pattern: &'static str) -> Self {
        Self {
            pattern,
            cell: OnceLock::new(),
        }
    }

    fn get(&self) -> &Regex {
        self.cell
            .get_or_init(|| Regex::new(self.pattern).expect("static placeholder regex"))
    }
}

impl std::ops::Deref for LazyRegex {
    type Target = Regex;
    fn deref(&self) -> &Regex {
        self.get()
    }
}

cached_regex!(JAVA_PTRN, r"\{java\}");
cached_regex!(OS_PTRN, r"\{os\}");
cached_regex!(LOCALE_PTRN, r"\{locale\}");
cached_regex!(AMPERSAND_DUPES_PTRN, r"&{2,}");
cached_regex!(PLACEHOLDER_PTRN, r"\{[^}]*\}");

fn ampersand_dupes_regex() -> &'static Regex {
    &AMPERSAND_DUPES_PTRN
}
fn placeholder_regex() -> &'static Regex {
    &PLACEHOLDER_PTRN
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------
