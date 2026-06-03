//! String case transformation.
//!
//! Port of Java `org.araymond.joal.core.client.emulated.utils.Casing`. The JSON
//! tags `upper` / `lower` / `none` are part of the `.client` file format and
//! must stay byte-compatible with the existing Java-produced JSON.

use serde::{Deserialize, Serialize};

/// Case transformation applied to a generated key or to the hex digits of a
/// URL-encoded byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Casing {
    #[serde(rename = "upper")]
    Upper,
    #[serde(rename = "lower")]
    Lower,
    #[serde(rename = "none")]
    None,
}

impl Casing {
    /// Apply this case transformation to `s`. Mirrors Java `Casing.toCase`.
    #[must_use]
    pub fn to_case(self, s: &str) -> String {
        match self {
            Casing::Upper => s.to_ascii_uppercase(),
            Casing::Lower => s.to_ascii_lowercase(),
            Casing::None => s.to_owned(),
        }
    }
}
