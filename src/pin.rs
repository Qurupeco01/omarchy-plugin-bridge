//! Pin lock state — what upstream is pinned to (CONCEPT §6).
//!
//! `upstream.lock` records `{channel, ref, commit}`. Only the `stable` channel
//! exists today (tag pinning); `master`/agile tracking is deferred. `channel`
//! is kept as data so the schema is forward-compatible.
#![allow(dead_code)] // consumed by bootstrap (C3+)

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const CHANNEL_STABLE: &str = "stable";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinLock {
    pub channel: String,
    /// Upstream ref as requested: tag or branch. Serialized as `ref`.
    #[serde(rename = "ref")]
    pub reference: String,
    /// Resolved full commit sha the pin dir was cloned at.
    pub commit: String,
}

impl PinLock {
    pub fn stable(reference: impl Into<String>, commit: impl Into<String>) -> Self {
        Self {
            channel: CHANNEL_STABLE.to_owned(),
            reference: reference.into(),
            commit: commit.into(),
        }
    }

    pub fn to_toml(&self) -> Result<String> {
        toml::to_string(self).context("serialize upstream.lock")
    }

    pub fn from_toml(s: &str) -> Result<Self> {
        toml::from_str(s).context("parse upstream.lock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let lock = PinLock::stable("v4.0.0", "0123456789abcdef0123456789abcdef01234567");
        let s = lock.to_toml().unwrap();
        assert_eq!(PinLock::from_toml(&s).unwrap(), lock);
    }

    #[test]
    fn serializes_ref_as_literal_ref_key() {
        let lock = PinLock::stable("v4.0.0", "abc");
        let s = lock.to_toml().unwrap();
        assert!(s.contains("ref = \"v4.0.0\""), "got: {s}");
    }

    #[test]
    fn channel_defaults_to_stable() {
        assert_eq!(PinLock::stable("x", "y").channel, CHANNEL_STABLE);
    }

    #[test]
    fn rejects_malformed_toml() {
        assert!(PinLock::from_toml("channel = 5").is_err());
    }
}