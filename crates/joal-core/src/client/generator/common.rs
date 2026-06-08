use std::fmt::Write as _;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rand::Rng;
use rand_regex::Regex as RandRegex;
use regex_syntax::ParserBuilder;

use crate::client::error::ClientError;

pub(super) const TORRENT_PERSISTENT_TTL: Duration = Duration::from_hours(2);

/// Compile a `.client` regex pattern into a `rand_regex::Regex` operating on
/// raw bytes. Byte-mode parsing (`unicode=false, utf8=false`) is required so
/// that high-byte escapes like `\xff` denote single bytes (not multi-byte
/// UTF-8 codepoints) — otherwise `[\x01-\xff]{12}` produces ~26 bytes instead
/// of 12 and trips the 20-byte peer-id integrity check.
///
/// Bundled `.client` files (e.g. `rtorrent-0.9.6_0.13.6.client`,
/// `bittorrent-7.10.3_44429.client`) embed literal high-byte chars such as
/// `\u{8d}` and `\u{ff}` as JSON escapes. By the time we see the pattern they
/// are full Unicode `char`s in the `String`, so we walk the string and rewrite
/// any codepoint in `0x80..=0xFF` to the equivalent `\xHH` regex literal. ASCII
/// codepoints pass through unchanged; anything beyond `0xFF` is rejected with
/// `InvalidRegex` because it cannot fit in a single wire byte.
pub(super) fn compile_rand_regex(pattern: &str) -> Result<RandRegex, ClientError> {
    let prepared = preprocess_pattern(pattern)?;
    let hir = ParserBuilder::new()
        .unicode(false)
        .utf8(false)
        .build()
        .parse(&prepared)
        .map_err(|e| ClientError::InvalidRegex(format!("{pattern}: {e}")))?;
    RandRegex::with_hir(hir, 100).map_err(|e| ClientError::InvalidRegex(format!("{pattern}: {e}")))
}

/// Sample raw peer-id/key bytes from a compiled `.client` regex pattern.
/// Shared by the REGEX peer-id and key algorithms, which differ only in the
/// surrounding trait family.
pub(super) fn sample_rand_regex<R: Rng + ?Sized>(
    pattern: &str,
    rng: &mut R,
) -> Result<Vec<u8>, ClientError> {
    let generator = compile_rand_regex(pattern)?;
    let bytes: Vec<u8> = rng.sample(&generator);
    Ok(bytes)
}

/// Rewrite codepoints `0x80..=0xFF` as `\xHH` escapes so the byte-mode parser
/// treats them as single literal bytes.
fn preprocess_pattern(pattern: &str) -> Result<String, ClientError> {
    let mut out = String::with_capacity(pattern.len());
    for ch in pattern.chars() {
        let code = u32::from(ch);
        if code <= 0x7F {
            out.push(ch);
        } else if code <= 0xFF {
            let _ = write!(out, "\\x{code:02x}");
        } else {
            return Err(ClientError::InvalidRegex(format!(
                "{pattern}: codepoint U+{code:04X} cannot be represented as a single peer-id byte"
            )));
        }
    }
    Ok(out)
}

pub(super) fn lock_state<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(super) fn default_shared_state<T: Default>() -> Arc<Mutex<T>> {
    Arc::new(Mutex::new(T::default()))
}

#[derive(Debug, Clone, Default)]
pub(super) struct TimedState {
    pub value: Option<Vec<u8>>,
    pub last_generation: Option<Instant>,
}

#[derive(Debug, Clone)]
pub(super) struct AccessAwareEntry {
    value: Vec<u8>,
    last_access: Instant,
}

impl AccessAwareEntry {
    pub fn new(value: Vec<u8>) -> Self {
        Self {
            value,
            last_access: Instant::now(),
        }
    }

    pub fn get(&mut self) -> &[u8] {
        self.last_access = Instant::now();
        &self.value
    }

    pub fn should_evict(&self, now: Instant) -> bool {
        now.duration_since(self.last_access) >= TORRENT_PERSISTENT_TTL
    }
}
