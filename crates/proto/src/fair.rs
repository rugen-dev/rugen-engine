//! "Bring your own character": content fingerprints, fairness profiles,
//! matchup verdicts and the matchup phase that runs before every online
//! match (pick → verdict → confirm → sync files → ready → relay session).
//!
//! Profiles are computed by `rugen-fair` on both sides (client at import
//! time, server again from the uploaded package), so the verdict is instant
//! and a client cannot lie about what its character does.

use serde::{Deserialize, Serialize};

use crate::{Rules, SessionTicket};

/// Version of the [`FairnessProfile`] analysis. Profiles of different
/// versions are never compared field by field (the server recomputes).
pub const FAIR_VERSION: u32 = 1;

/// Largest card portrait (base64 WebP) a client may attach.
pub const MAX_PORTRAIT_B64: usize = 24 * 1024;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Shown and penalised in the power score, still playable.
    #[default]
    Warn,
    /// Never allowed online.
    Block,
}

/// One finding of the static CNS analysis.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct CheatFlag {
    /// Stable machine code, e.g. `regen`, `permanent_invuln`, `target_life_set`.
    pub code: String,
    pub severity: Severity,
    /// Human-readable detail (`state -2: LifeAdd value = 5 every frame`).
    pub detail: String,
}

/// Gameplay summary of a character, computed from its files.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FairnessProfile {
    pub version: u32,
    pub life: i32,
    pub power_max: i32,
    pub attack: i32,
    pub defence: i32,
    /// Velocities normalised to a 320-wide localcoord.
    pub walk_fwd: f32,
    pub run_fwd: f32,
    pub jump_y: f32,
    pub states: u32,
    pub hitdefs: u32,
    pub max_hit_damage: i32,
    /// Rough: best damage sum along chained (hit-confirm / cancel) attack states.
    pub est_combo_damage: i32,
    pub supers: u32,
    /// Cheapest super in power units (0 = no super).
    pub min_super_cost: i32,
    pub throws: u32,
    /// Hits without a guard flag (excluding throws).
    pub unblockable: u32,
    pub projectiles: u32,
    pub helpers: u32,
    /// Longest NotHitBy / HitBy window inside an attack state, in frames.
    pub invuln_frames: u32,
    pub flags: Vec<CheatFlag>,
    /// `rugen verify` verdict (`verified` / `warn` / `broken`) when known;
    /// client-side only, never part of a server comparison.
    pub verify: Option<String>,
    /// ≈100 for Kung Fu Man.
    pub power_score: f32,
    pub reasons: Vec<String>,
}

impl FairnessProfile {
    pub fn blocked(&self) -> bool {
        self.flags.iter().any(|f| f.severity == Severity::Block)
            || self.verify.as_deref() == Some("broken")
    }

    /// The profile without client-only fields, for "did the client lie" checks.
    pub fn comparable(&self) -> Self {
        Self {
            verify: None,
            ..self.clone()
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerdictKind {
    #[default]
    Fair,
    /// Allowed in casual rooms when both players accept, never in ranked.
    Unbalanced,
    /// Never allowed.
    Blocked,
}

impl VerdictKind {
    pub fn label(self) -> &'static str {
        match self {
            VerdictKind::Fair => "FAIR",
            VerdictKind::Unbalanced => "UNBALANCED",
            VerdictKind::Blocked => "BLOCKED",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Verdict {
    pub kind: VerdictKind,
    pub reasons: Vec<String>,
    /// Stronger / weaker power score.
    #[serde(default)]
    pub ratio: f32,
}

/// Per-fingerprint community rating (Elo between characters, from verified results).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct CharRating {
    pub rating: i32,
    pub games: u32,
    pub wins: u32,
}

/// What a client declares for a fighter (or the stage) it brings.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FighterDecl {
    /// sha256 hex of the canonical gameplay files (see `rugen-fair`).
    pub fingerprint: String,
    /// `char` or `stage`.
    pub kind: String,
    /// Local id (file stem), informative.
    pub id: String,
    pub name: String,
    pub author: String,
    /// Main .def inside the package.
    pub def: String,
    /// Optimised package size in bytes.
    pub size: u64,
    pub profile: Option<FairnessProfile>,
    /// Small WebP card portrait, base64.
    pub portrait: Option<String>,
}

/// A fighter as the server shows it to both players.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct FighterCard {
    pub decl: FighterDecl,
    pub rating: Option<CharRating>,
    /// The server has the package and recomputed fingerprint + profile.
    pub verified: bool,
}

/// `HAS <fingerprint>` answer and upload progress.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PackageStatus {
    pub fingerprint: String,
    /// Stored and verified on the server.
    pub complete: bool,
    pub size: u64,
    /// Bytes of this account's partial upload (resume from here).
    pub offset: u64,
}

/// What a client reports once it holds every file of the match.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ReadyReport {
    pub p1: String,
    pub p2: String,
    pub stage: String,
    /// Game version (`rugen 0.2.0`); both sides must match.
    pub engine: String,
    pub rules: Rules,
}

/// Matchup phase of one match, as seen by one member.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MatchupView {
    /// Same as the match id results are reported under.
    pub id: u64,
    pub ranked: bool,
    pub rules: Rules,
    /// 1 = P1, 2 = P2, 3 = spectator.
    pub side: u8,
    /// `pick`, `confirm`, `sync`, `started` or `failed`.
    pub phase: String,
    pub fighters: [Option<FighterCard>; 2],
    pub stage: Option<FighterCard>,
    pub verdict: Option<Verdict>,
    pub accepted: [bool; 2],
    pub ready: [bool; 2],
    /// Fingerprints the server still needs uploaded.
    pub missing: Vec<String>,
    /// Why the matchup failed / why confirming is refused.
    pub error: String,
    /// The P2 fighter is P1's (room rule).
    pub mirror: bool,
    /// Relay ticket once the match started.
    pub ticket: Option<SessionTicket>,
    /// The verdict as a [`crate::VerdictStatement`] signed by the server (API 4).
    pub signed: Option<crate::SignedBlob>,
}

impl MatchupView {
    /// Fingerprints of P1, P2 and the stage once known.
    pub fn fingerprints(&self) -> [Option<&str>; 3] {
        fn fp(c: &Option<FighterCard>) -> Option<&str> {
            c.as_ref().map(|c| c.decl.fingerprint.as_str())
        }
        [
            fp(&self.fighters[0]),
            fp(&self.fighters[1]),
            fp(&self.stage),
        ]
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding (card portraits).
pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for ch in s.bytes() {
        let v = B64.iter().position(|&b| b == ch)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Is `s` a lowercase sha256 hex fingerprint?
pub fn is_fingerprint(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip() {
        for n in 0..20 {
            let data: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(b64_decode(&b64_encode(&data)).unwrap(), data);
        }
        assert_eq!(b64_encode(b"Man"), "TWFu");
        assert_eq!(b64_encode(b"Ma"), "TWE=");
        assert!(b64_decode("@@").is_none());
    }

    #[test]
    fn verdict_json_is_snake_case() {
        let v = Verdict {
            kind: VerdictKind::Unbalanced,
            reasons: vec!["x".into()],
            ratio: 2.0,
        };
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.contains(r#""kind":"unbalanced""#), "{s}");
        assert!(is_fingerprint(&"a".repeat(64)));
        assert!(!is_fingerprint(&"A".repeat(64)));
    }
}
