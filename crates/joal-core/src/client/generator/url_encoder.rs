//! Byte-level URL encoder used by the tracker announce query builder.
//!
//! Port of Java `org.araymond.joal.core.client.emulated.generator.UrlEncoder`.
//! The serialized shape stays Java-compatible; the Rust implementation also
//! exposes `encode_bytes` so raw `info_hash` bytes can be encoded directly.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::client::error::ClientError;
use crate::client::utils::Casing;

/// Byte-level URL encoder. Serialized identically to Java's
/// `UrlEncoder` (`encodingExclusionPattern` + `encodedHexCase`), and compares
/// equal on those two fields alone (the cached compiled [`Regex`] is elided).
#[derive(Debug, Serialize, Deserialize)]
pub struct UrlEncoder {
    #[serde(rename = "encodingExclusionPattern")]
    encoding_exclusion_pattern: String,
    #[serde(rename = "encodedHexCase")]
    encoded_hex_case: Casing,
    /// Lazily compiled regex anchored with `\A...\z`, to mirror Java
    /// `Pattern.matcher(...).matches()` full-input matching semantics.
    #[serde(skip)]
    compiled: OnceLock<Regex>,
}

impl Clone for UrlEncoder {
    fn clone(&self) -> Self {
        Self {
            encoding_exclusion_pattern: self.encoding_exclusion_pattern.clone(),
            encoded_hex_case: self.encoded_hex_case,
            compiled: OnceLock::new(),
        }
    }
}

impl PartialEq for UrlEncoder {
    fn eq(&self, other: &Self) -> bool {
        self.encoding_exclusion_pattern == other.encoding_exclusion_pattern
            && self.encoded_hex_case == other.encoded_hex_case
    }
}

impl Eq for UrlEncoder {}

impl UrlEncoder {
    pub fn new(pattern: impl Into<String>, encoded_hex_case: Casing) -> Result<Self, ClientError> {
        let encoder = Self {
            encoding_exclusion_pattern: pattern.into(),
            encoded_hex_case,
            compiled: OnceLock::new(),
        };
        encoder.validate()?;
        Ok(encoder)
    }

    /// Exclusion pattern as it appears in the `.client` file.
    #[must_use]
    pub fn encoding_exclusion_pattern(&self) -> &str {
        &self.encoding_exclusion_pattern
    }

    /// Casing applied to the hex digits of `%HH`-encoded bytes.
    #[must_use]
    pub fn encoded_hex_case(&self) -> Casing {
        self.encoded_hex_case
    }

    pub fn validate(&self) -> Result<(), ClientError> {
        self.pattern().map(|_| ())
    }

    fn pattern(&self) -> Result<&Regex, ClientError> {
        if let Some(r) = self.compiled.get() {
            return Ok(r);
        }
        let anchored = format!(r"\A(?:{})\z", self.encoding_exclusion_pattern);
        let compiled = Regex::new(&anchored).map_err(|e| {
            ClientError::InvalidRegex(format!("{}: {e}", self.encoding_exclusion_pattern))
        })?;
        // Losing the race to another thread is fine — both compile an equivalent regex.
        Ok(self.compiled.get_or_init(|| compiled))
    }

    /// Encode `s` as-if by the tracker announce URL rule set.
    ///
    /// Returns `Err` only on first use if the pattern fails to compile; once
    /// compilation succeeded no subsequent call can fail.
    pub fn encode(&self, s: &str) -> Result<String, ClientError> {
        self.encode_bytes(s.as_bytes())
    }

    /// Byte-slice variant; identical output for ASCII input, but allows raw
    /// 20-byte `info_hash` values to flow through with byte-level fidelity.
    pub fn encode_bytes(&self, bytes: &[u8]) -> Result<String, ClientError> {
        let pattern = self.pattern()?;
        let mut out = String::with_capacity(bytes.len() * 3);
        for &b in bytes {
            self.append_encoded_byte(pattern, b, &mut out);
        }
        Ok(out)
    }

    fn append_encoded_byte(&self, pattern: &Regex, b: u8, out: &mut String) {
        // Only ASCII bytes can match the exclusion pattern — non-ASCII bytes
        // are not valid UTF-8 on their own so we treat them as always-encoded.
        if b.is_ascii() {
            let buf = [b];
            if let Ok(s) = std::str::from_utf8(&buf)
                && pattern.is_match(s)
            {
                out.push(b as char);
                return;
            }
        }

        // Java special-cases `0` to the literal `"%00"`; since `format!("%{:02x}", 0)`
        // produces the same five bytes, we fold it into the general path.
        let hex = format!("%{b:02x}");
        out.push_str(&self.encoded_hex_case.to_case(&hex));
    }
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Same exclusion pattern as `resources/clients/qbittorrent-4.5.0.client`.
    const QB_PATTERN: &str = r"[A-Za-z0-9_~\(\)\!\.\*-]";

    #[test]
    fn lower_hex_passes_through_unreserved_chars_untouched() {
        let enc = UrlEncoder::new(QB_PATTERN, Casing::Lower).unwrap();
        assert_eq!(enc.encode("Hello.World-42").unwrap(), "Hello.World-42");
    }

    #[test]
    fn lower_hex_encodes_reserved_bytes() {
        let enc = UrlEncoder::new(QB_PATTERN, Casing::Lower).unwrap();
        assert_eq!(enc.encode(" ").unwrap(), "%20");
        assert_eq!(enc.encode("/").unwrap(), "%2f");
        assert_eq!(enc.encode("?").unwrap(), "%3f");
    }

    #[test]
    fn upper_hex_casing_uppercases_hex_digits() {
        let enc = UrlEncoder::new(QB_PATTERN, Casing::Upper).unwrap();
        assert_eq!(enc.encode("/").unwrap(), "%2F");
        assert_eq!(enc.encode("\x00").unwrap(), "%00");
    }

    #[test]
    fn zero_byte_encodes_as_percent_00() {
        let enc = UrlEncoder::new(QB_PATTERN, Casing::Lower).unwrap();
        assert_eq!(enc.encode_bytes(&[0]).unwrap(), "%00");
    }

    #[test]
    fn non_ascii_bytes_always_encode() {
        // Even with a permissive pattern like `.`, non-ASCII bytes never get
        // expanded to ASCII; they are always emitted as `%HH`.
        let enc = UrlEncoder::new(r"[\x00-\xff]", Casing::Lower).unwrap();
        // 0x80 isn't valid UTF-8 on its own → cannot match ASCII exclusion.
        assert_eq!(enc.encode_bytes(&[0x80]).unwrap(), "%80");
    }

    #[test]
    fn invalid_pattern_rejected_eagerly() {
        let err = UrlEncoder::new("[unclosed", Casing::Lower).unwrap_err();
        assert!(matches!(err, ClientError::InvalidRegex(_)));
    }

    #[test]
    fn serde_field_names_match_java_client_format() {
        let json = r#"{"encodingExclusionPattern":"[A-Za-z]","encodedHexCase":"lower"}"#;
        let enc: UrlEncoder = serde_json::from_str(json).unwrap();
        assert_eq!(enc.encoding_exclusion_pattern(), "[A-Za-z]");
        assert_eq!(enc.encoded_hex_case(), Casing::Lower);

        let roundtrip = serde_json::to_string(&enc).unwrap();
        // Order is stable — serde_json emits struct fields in declaration order.
        assert_eq!(
            roundtrip,
            r#"{"encodingExclusionPattern":"[A-Za-z]","encodedHexCase":"lower"}"#
        );
    }

    #[test]
    fn partial_eq_ignores_compiled_cache() {
        let a = UrlEncoder::new(QB_PATTERN, Casing::Lower).unwrap();
        let b = UrlEncoder::new(QB_PATTERN, Casing::Lower).unwrap();
        // warm only one side's cache
        let _ = a.encode("x").unwrap();
        assert_eq!(a, b);
    }

    /// Exhaustive byte-range snapshot for the qBittorrent 4.5.0 exclusion
    /// pattern. Locks in the S4 promise: every byte from `0x00` through
    /// `0xFF` encodes to a specific, tracker-visible output.
    #[test]
    fn byte_range_snapshot_matches_java_semantics() {
        let enc = UrlEncoder::new(QB_PATTERN, Casing::Lower).unwrap();
        // Excluded (pass-through) bytes per the qB 4.5.0 pattern.
        // [A-Za-z0-9_~\(\)\!\.\*-]
        let pass_through: &[u8] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_~()!.*-";

        for byte in 0u8..=255u8 {
            let got = enc.encode_bytes(&[byte]).unwrap();
            if pass_through.contains(&byte) {
                assert_eq!(
                    got,
                    (byte as char).to_string(),
                    "byte 0x{byte:02x} should pass through"
                );
            } else {
                assert_eq!(got, format!("%{byte:02x}"), "byte 0x{byte:02x} must %HH");
            }
        }
    }
}
