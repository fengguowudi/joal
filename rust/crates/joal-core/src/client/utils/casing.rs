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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upper_uppercases_ascii() {
        assert_eq!(Casing::Upper.to_case("Hello 42"), "HELLO 42");
    }

    #[test]
    fn lower_lowercases_ascii() {
        assert_eq!(Casing::Lower.to_case("Hello 42"), "hello 42");
    }

    #[test]
    fn none_is_identity() {
        assert_eq!(Casing::None.to_case("AbC"), "AbC");
    }

    #[test]
    fn json_tags_match_java() {
        assert_eq!(serde_json::to_string(&Casing::Upper).unwrap(), "\"upper\"");
        assert_eq!(serde_json::to_string(&Casing::Lower).unwrap(), "\"lower\"");
        assert_eq!(serde_json::to_string(&Casing::None).unwrap(), "\"none\"");

        let up: Casing = serde_json::from_str("\"upper\"").unwrap();
        assert_eq!(up, Casing::Upper);
        let lo: Casing = serde_json::from_str("\"lower\"").unwrap();
        assert_eq!(lo, Casing::Lower);
        let nn: Casing = serde_json::from_str("\"none\"").unwrap();
        assert_eq!(nn, Casing::None);
    }
}
