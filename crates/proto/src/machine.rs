//! A player's machine in a few words, for debug reports: the game version
//! and build, OS, CPU architecture and graphics backend. Peers exchange it
//! (LAN handshake, relay session) and clients send it with their ready
//! report so each side's replay can say what the other ran.
//!
//! Privacy rule: never a hostname, user name, address, serial number or
//! path; the fields are short public facts and [`MachineSummary::clean`]
//! caps them.

use serde::{Deserialize, Serialize};

/// Longest field kept (characters).
pub const FIELD_MAX: usize = 48;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MachineSummary {
    /// `rugen 0.7.0`
    pub engine: String,
    /// Build commit (git short hash, `unknown` without git).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub commit: String,
    /// `Fedora Linux 44`, `Windows 11 (10.0.22631)`
    pub os: String,
    /// `x86_64`, `aarch64`
    pub arch: String,
    /// wgpu backend (`Vulkan`, `Metal`, `Dx12`, `Gl`) or `headless`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub backend: String,
}

impl MachineSummary {
    /// Fields capped to [`FIELD_MAX`] printable characters (what a peer
    /// sent is never trusted to be short or clean).
    pub fn clean(mut self) -> Self {
        for f in [
            &mut self.engine,
            &mut self.commit,
            &mut self.os,
            &mut self.arch,
            &mut self.backend,
        ] {
            *f = f
                .chars()
                .filter(|c| !c.is_control())
                .take(FIELD_MAX)
                .collect::<String>()
                .trim()
                .to_string();
        }
        self
    }

    /// What a shared replay keeps: the game version and build, OS and
    /// architecture (no graphics details).
    pub fn shared(&self) -> Self {
        Self {
            backend: String::new(),
            ..self.clone()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.engine.is_empty() && self.os.is_empty() && self.arch.is_empty()
    }

    /// One line: `rugen 0.7.0 (1d5573e) · Fedora Linux 44 · x86_64 · Vulkan`.
    pub fn line(&self) -> String {
        let mut engine = self.engine.clone();
        if !self.commit.is_empty() && self.commit != "unknown" {
            engine = format!("{engine} ({})", self.commit);
        }
        [engine, self.os.clone(), self.arch.clone(), self.backend.clone()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// Compact JSON for a datagram (LAN hello, relay message).
    pub fn encode(&self) -> String {
        serde_json::to_string(&self.clone().clean()).unwrap_or_default()
    }

    /// A peer's [`encode`](Self::encode)d summary; `None` when it is not one.
    pub fn decode(s: &str) -> Option<Self> {
        if s.len() > 1024 {
            return None;
        }
        serde_json::from_str::<Self>(s)
            .ok()
            .map(Self::clean)
            .filter(|m| !m.is_empty())
    }
}

/// `skip_serializing_if` for a pair of optional summaries.
pub fn both_none(m: &[Option<MachineSummary>; 2]) -> bool {
    m.iter().all(Option::is_none)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_travel_short_and_clean() {
        let m = MachineSummary {
            engine: "rugen 0.7.0".into(),
            commit: "1d5573e".into(),
            os: "Fedora Linux 44".into(),
            arch: "x86_64".into(),
            backend: "Vulkan".into(),
        };
        assert_eq!(MachineSummary::decode(&m.encode()), Some(m.clone()));
        assert_eq!(
            m.line(),
            "rugen 0.7.0 (1d5573e) · Fedora Linux 44 · x86_64 · Vulkan"
        );
        let shared = m.shared();
        assert!(shared.backend.is_empty());
        assert!(!serde_json::to_string(&shared).unwrap().contains("backend"));
        // a hostile peer: long and with control characters
        let evil = format!(r#"{{"engine":"{}\n\u0007","os":"x"}}"#, "A".repeat(500));
        let got = MachineSummary::decode(&evil).unwrap();
        assert_eq!(got.engine.len(), FIELD_MAX);
        assert!(MachineSummary::decode("not json").is_none());
        assert!(MachineSummary::decode("{}").is_none(), "empty is nothing");
    }
}
