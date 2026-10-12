//! Signed statements of a server: the official truth feed (global cheat
//! blocklist + community ratings), matchup verdicts, ranked results and the
//! identity proof a client asks for when it probes a server.
//!
//! Every statement travels inside a [`SignedBlob`]: the payload is the
//! statement's *canonical JSON* text and the signature covers exactly those
//! bytes (see `rugen_fair::trust` for the canonical form, the signed message
//! layout and verification). Only a blob signed by the key baked into the
//! game build is ever shown as OFFICIAL; other servers may sign with their
//! own key and are shown as COMMUNITY with that key's fingerprint.

use serde::{Deserialize, Serialize};

use crate::VerdictKind;

/// Version of the statement formats below.
pub const TRUST_VERSION: u32 = 1;

/// `purpose` of a [`SignedBlob`]: what the payload is (domain separation,
/// a verdict signature can never pass as a feed and so on).
pub mod purpose {
    pub const TRUTH: &str = "truth";
    pub const VERDICT: &str = "verdict";
    pub const RESULT: &str = "result";
    pub const IDENTITY: &str = "identity";
}

/// A statement and its Ed25519 signature.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedBlob {
    /// One of [`purpose`].
    pub purpose: String,
    /// Canonical JSON of the statement (the signed bytes).
    pub payload: String,
    /// Signer's Ed25519 public key, 64 hex digits.
    pub key: String,
    /// Ed25519 signature, 128 hex digits.
    pub sig: String,
}

/// One blocked character / stage fingerprint.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct BlockEntry {
    pub fingerprint: String,
    pub reason: String,
    /// Unix seconds it was blocked.
    pub since: u64,
    /// `admin` (operator) or `auto` (server analysis, liars, desyncs).
    pub source: String,
}

/// Community rating of a fingerprint on the server publishing the feed.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct FeedRating {
    pub fingerprint: String,
    pub rating: i32,
    pub games: u32,
    pub wins: u32,
}

/// `GET /v1/truth`: the publishing server's blocklist and ratings.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TruthFeed {
    pub v: u32,
    /// Server name (informative).
    pub server: String,
    /// Unix seconds this copy was signed.
    pub issued_at: u64,
    /// Unix seconds after which the feed is stale and must be ignored.
    pub expires_at: u64,
    /// Grows with every blocklist change: a client or server never accepts
    /// a feed older than the one it already holds (no rollback).
    pub sequence: u64,
    pub blocked: Vec<BlockEntry>,
    pub ratings: Vec<FeedRating>,
}

impl TruthFeed {
    pub fn blocked(&self, fingerprint: &str) -> Option<&BlockEntry> {
        self.blocked.iter().find(|b| b.fingerprint == fingerprint)
    }

    pub fn rating(&self, fingerprint: &str) -> Option<&FeedRating> {
        self.ratings.iter().find(|r| r.fingerprint == fingerprint)
    }
}

/// The verdict of one matchup, signed by the server that judged it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct VerdictStatement {
    pub v: u32,
    pub match_id: u64,
    pub ranked: bool,
    /// P1, P2 and (when picked) stage fingerprints.
    pub fingerprints: Vec<String>,
    pub verdict: VerdictKind,
    pub reasons: Vec<String>,
    pub issued_at: u64,
}

/// A settled result as seen by one player, signed by the server.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ResultStatement {
    pub v: u32,
    pub match_id: u64,
    pub ranked: bool,
    /// The account this statement is for.
    pub account: u64,
    pub players: [u64; 2],
    pub winner: u64,
    /// The account's rating after the match and its change (ranked).
    pub rating: Option<i32>,
    pub delta: Option<i32>,
    pub season: u32,
    pub issued_at: u64,
}

/// `GET /v1/identity?nonce=`: proof that the server holds its key right now.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct IdentityStatement {
    pub v: u32,
    /// The client's random challenge, echoed.
    pub nonce: String,
    pub name: String,
    /// The server believes it is the official one (clients only trust the key).
    pub official: bool,
    pub issued_at: u64,
}
