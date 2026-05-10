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

use std::net::IpAddr;
use std::sync::OnceLock;

use regex::Regex;

use crate::client::config::{BitTorrentClientConfig, HttpHeader};
use crate::client::error::ClientError;
use crate::client::event::RequestEvent;
use crate::client::generator::{KeyGenerator, NumwantProvider, PeerIdGenerator, UrlEncoder};
use crate::client::runtime::{ConnectionHandler, TorrentSeedStats};
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
        let headers = resolve_request_headers(&request_headers)?;

        Ok(Self {
            peer_id_generator,
            key_generator,
            url_encoder,
            numwant_provider,
            query,
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
    /// configured refresh policy.
    pub fn peer_id(
        &self,
        info_hash: &InfoHash,
        event: RequestEvent,
    ) -> Result<String, ClientError> {
        self.peer_id_generator.get(info_hash, event)
    }

    /// Returns `None` when the `.client` config has no `keyGenerator`.
    pub fn key(
        &self,
        info_hash: &InfoHash,
        event: RequestEvent,
    ) -> Result<Option<String>, ClientError> {
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
        let info_hash_encoded = self.url_encoder.encode_bytes(info_hash.as_bytes())?;
        let mut q = infohash_regex()
            .replace_all(&self.query, regex::NoExpand(&info_hash_encoded))
            .into_owned();

        q = replace_literal(&q, &UPLOADED_PTRN, &stats.uploaded.to_string());
        q = replace_literal(&q, &DOWNLOADED_PTRN, &stats.downloaded.to_string());
        q = replace_literal(&q, &LEFT_PTRN, &stats.left.to_string());
        q = replace_literal(&q, &PORT_PTRN, &connection.port().to_string());
        q = replace_literal(&q, &NUMWANT_PTRN, &self.numwant(event).to_string());

        let peer_id_raw = self.peer_id(info_hash, event)?;
        let peer_id = if self.peer_id_generator.should_url_encode() {
            self.url_encoder.encode(&peer_id_raw)?
        } else {
            peer_id_raw
        };
        q = replace_literal(&q, &PEER_ID_PTRN, &peer_id);

        match connection.ip_address() {
            IpAddr::V4(v4) if q.contains("{ip}") => {
                q = replace_literal(&q, &IP_PTRN, &v4.to_string());
            }
            IpAddr::V6(v6) if q.contains("{ipv6}") => {
                let encoded = self.url_encoder.encode(&v6.to_string())?;
                q = replace_literal(&q, &IPV6_PTRN, &encoded);
            }
            _ => {}
        }
        // Remove any leftover `&key={ip}` / `&key={ipv6}` pairs whose host
        // family did not match; same as Java's `IP_Q_PTRN` sweep.
        q = ip_qpair_regex().replace_all(&q, "").into_owned();

        if event == RequestEvent::None {
            q = event_qpair_regex().replace_all(&q, "").into_owned();
        } else {
            q = replace_literal(&q, &EVENT_PTRN, event.event_name());
        }

        if q.contains("{key}") {
            let key = self.key(info_hash, event)?.ok_or_else(|| {
                ClientError::Integrity(
                    "Client request query contains 'key' but BitTorrentClient does not have a key"
                        .to_owned(),
                )
            })?;
            let encoded = self.url_encoder.encode(&key)?;
            q = replace_literal(&q, &KEY_PTRN, &encoded);
        }

        if let Some(m) = placeholder_regex().find(&q) {
            return Err(ClientError::Integrity(format!(
                "Placeholder [{}] were not recognized while building announce URL",
                m.as_str()
            )));
        }

        Ok(trim_ampersands(&collapse_ampersands(&q)).to_owned())
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

cached_regex!(INFOHASH_PTRN, r"\{infohash\}");
cached_regex!(UPLOADED_PTRN, r"\{uploaded\}");
cached_regex!(DOWNLOADED_PTRN, r"\{downloaded\}");
cached_regex!(LEFT_PTRN, r"\{left\}");
cached_regex!(PORT_PTRN, r"\{port\}");
cached_regex!(NUMWANT_PTRN, r"\{numwant\}");
cached_regex!(PEER_ID_PTRN, r"\{peerid\}");
cached_regex!(EVENT_PTRN, r"\{event\}");
cached_regex!(KEY_PTRN, r"\{key\}");
cached_regex!(JAVA_PTRN, r"\{java\}");
cached_regex!(OS_PTRN, r"\{os\}");
cached_regex!(LOCALE_PTRN, r"\{locale\}");
cached_regex!(IP_PTRN, r"\{ip\}");
cached_regex!(IPV6_PTRN, r"\{ipv6\}");
cached_regex!(AMPERSAND_DUPES_PTRN, r"&{2,}");
cached_regex!(IP_QPAIR_PTRN, r"&*\w+=\{ip(?:v6)?\}");
cached_regex!(EVENT_QPAIR_PTRN, r"&*\w+=\{event\}");
cached_regex!(PLACEHOLDER_PTRN, r"\{[^}]*\}");

fn infohash_regex() -> &'static Regex {
    &INFOHASH_PTRN
}
fn ampersand_dupes_regex() -> &'static Regex {
    &AMPERSAND_DUPES_PTRN
}
fn ip_qpair_regex() -> &'static Regex {
    &IP_QPAIR_PTRN
}
fn event_qpair_regex() -> &'static Regex {
    &EVENT_QPAIR_PTRN
}
fn placeholder_regex() -> &'static Regex {
    &PLACEHOLDER_PTRN
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn qb_config() -> BitTorrentClientConfig {
        let json = r#"{
            "peerIdGenerator": {
                "refreshOn": "NEVER",
                "algorithm": {"type": "REGEX", "pattern": "-qB4500-[A-Za-z0-9]{12}"},
                "shouldUrlEncode": false
            },
            "keyGenerator": {
                "refreshOn": "ALWAYS",
                "algorithm": {"type": "HASH_NO_LEADING_ZERO", "length": 8},
                "keyCase": "upper"
            },
            "urlEncoder": {
                "encodingExclusionPattern": "[A-Za-z0-9_~\\(\\)\\!\\.\\*-]",
                "encodedHexCase": "lower"
            },
            "query": "info_hash={infohash}&peer_id={peerid}&port={port}&uploaded={uploaded}&downloaded={downloaded}&left={left}&key={key}&event={event}&numwant={numwant}",
            "requestHeaders": [
                {"name": "User-Agent", "value": "qBittorrent/4.5.0"}
            ],
            "numwant": 200,
            "numwantOnStop": 0
        }"#;
        BitTorrentClientConfig::try_from(json).unwrap()
    }

    fn sample_info_hash() -> InfoHash {
        let mut b = [0u8; 20];
        for (i, byte) in b.iter_mut().enumerate() {
            *byte = i as u8;
        }
        InfoHash::from_bytes(b)
    }

    fn connection() -> ConnectionHandler {
        ConnectionHandler::new(55555, IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)))
    }

    #[test]
    fn query_collapses_consecutive_ampersands() {
        let mut cfg = qb_config();
        cfg.query = "a=1&&b=2&&&c=3".to_owned();
        let client = BitTorrentClient::new(cfg).unwrap();
        assert_eq!(client.query(), "a=1&b=2&c=3");
    }

    #[test]
    fn rejects_blank_query() {
        let mut cfg = qb_config();
        cfg.query = "   ".to_owned();
        assert!(matches!(
            BitTorrentClient::new(cfg),
            Err(ClientError::Integrity(_))
        ));
    }

    #[test]
    fn headers_resolve_process_placeholders() {
        let mut cfg = qb_config();
        cfg.request_headers[0].value = "JOAL/{java} ({os}; {locale})".to_owned();
        let client = BitTorrentClient::new(cfg).unwrap();
        let (_, v) = &client.headers()[0];
        assert!(
            !v.contains('{'),
            "unresolved placeholder in header value: {v}"
        );
        assert!(v.contains(DEFAULT_JAVA_VERSION));
        assert!(v.contains(std::env::consts::OS));
    }

    #[test]
    fn header_unknown_placeholder_is_integrity_error() {
        let mut cfg = qb_config();
        cfg.request_headers[0].value = "x-{bogus}-y".to_owned();
        let err = BitTorrentClient::new(cfg).unwrap_err();
        assert!(matches!(err, ClientError::Integrity(_)));
    }

    #[test]
    fn create_request_query_removes_event_on_none() {
        let client = BitTorrentClient::new(qb_config()).unwrap();
        let stats = TorrentSeedStats::new(111, 222, 333);
        let q = client
            .create_request_query(
                RequestEvent::None,
                &sample_info_hash(),
                &stats,
                &connection(),
            )
            .unwrap();
        assert!(
            !q.contains("event="),
            "none event should drop key=value: {q}"
        );
        assert!(!q.contains('{'), "no placeholders may survive: {q}");
        assert!(q.contains("numwant=200"));
        assert!(q.contains("port=55555"));
    }

    #[test]
    fn create_request_query_encodes_info_hash_bytes() {
        let client = BitTorrentClient::new(qb_config()).unwrap();
        let stats = TorrentSeedStats::new(0, 0, 0);
        let q = client
            .create_request_query(
                RequestEvent::Started,
                &sample_info_hash(),
                &stats,
                &connection(),
            )
            .unwrap();
        // info_hash bytes 0x00..0x13 all fall outside the qBittorrent pass-through set.
        let expected = "info_hash=%00%01%02%03%04%05%06%07%08%09%0a%0b%0c%0d%0e%0f%10%11%12%13";
        assert!(
            q.contains(expected),
            "query missing expected info_hash segment: {q}"
        );
        assert!(q.contains("event=started"));
        assert!(q.contains("numwant=200"));
    }

    #[test]
    fn create_request_query_uses_numwant_on_stop() {
        let client = BitTorrentClient::new(qb_config()).unwrap();
        let stats = TorrentSeedStats::new(0, 0, 0);
        let q = client
            .create_request_query(
                RequestEvent::Stopped,
                &sample_info_hash(),
                &stats,
                &connection(),
            )
            .unwrap();
        assert!(q.contains("event=stopped"));
        assert!(q.contains("numwant=0"));
    }

    #[test]
    fn missing_key_generator_when_query_needs_key_errors_at_announce_time() {
        // Build by hand a client whose query uses {key} but whose key
        // generator is None — normal validation would reject this, so we
        // construct `BitTorrentClient` directly to bypass it and verify the
        // announce-time guard still fires.
        let cfg = qb_config();
        let url_encoder = cfg.url_encoder.clone();
        let peer_id_generator = cfg.peer_id_generator.clone();
        let client = BitTorrentClient {
            peer_id_generator,
            key_generator: None,
            url_encoder,
            numwant_provider: NumwantProvider::new(200, 0).unwrap(),
            query: "info_hash={infohash}&key={key}&port={port}".to_owned(),
            headers: Vec::new(),
        };
        let err = client
            .create_request_query(
                RequestEvent::Started,
                &sample_info_hash(),
                &TorrentSeedStats::new(0, 0, 0),
                &connection(),
            )
            .unwrap_err();
        assert!(matches!(err, ClientError::Integrity(msg) if msg.contains("'key'")));
    }
}
