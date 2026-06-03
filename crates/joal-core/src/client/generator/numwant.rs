//! `numwant` URL parameter provider.
//!
//! Port of Java
//! `org.araymond.joal.core.client.emulated.generator.numwant.NumwantProvider`.
//! Chooses between two constants based on whether the announce is a
//! regular/started announce or the final `stopped` announce at shutdown.

use serde::{Deserialize, Serialize};

use crate::client::error::ClientError;
use crate::client::event::RequestEvent;

/// Two-value numwant policy. `numwant` is sent on every announce except
/// `stopped`, where `numwant_on_stop` is used instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumwantProvider {
    pub numwant: i32,
    #[serde(rename = "numwantOnStop")]
    pub numwant_on_stop: i32,
}

impl NumwantProvider {
    /// Validates inputs exactly like Java's constructor preconditions.
    pub fn new(numwant: i32, numwant_on_stop: i32) -> Result<Self, ClientError> {
        if numwant < 1 {
            return Err(ClientError::Integrity(
                "numwant must be at least 1".to_owned(),
            ));
        }
        if numwant_on_stop < 0 {
            return Err(ClientError::Integrity(
                "numwantOnStop must be at least 0".to_owned(),
            ));
        }
        Ok(Self {
            numwant,
            numwant_on_stop,
        })
    }

    /// Return the numwant value appropriate for `event`.
    #[must_use]
    pub fn get(self, event: RequestEvent) -> i32 {
        if event == RequestEvent::Stopped {
            self.numwant_on_stop
        } else {
            self.numwant
        }
    }
}
