//! The official rugen server as the client sees it.
//!
//! * Its address and Ed25519 public key are baked into the build
//!   ([`rugen_fair::trust::OFFICIAL_SERVER`] / `OFFICIAL_KEY`, set with
//!   `RUGEN_OFFICIAL_SERVER` / `RUGEN_OFFICIAL_KEY` at compile time). It is
//!   always first in the ONLINE list and cannot be removed.
//! * A server only counts as OFFICIAL once it signed a random nonce of ours
//!   with that key ([`identity_trust`]); every other server is COMMUNITY
//!   (shown with its own key fingerprint when it signs).
//! * The official truth feed (global blocklist + community ratings) is
//!   verified, freshness-checked and cached in `cache/official_truth.json`
//!   ([`store_feed`] / [`feed`]); the roster and matchup cards use it.
//! * Verdicts and results carry signatures ([`verdict_stamp`],
//!   [`result_stamp`]); an OFFICIAL stamp needs the official key, a bad
//!   signature is reported as forged and ignored.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rugen_fair::trust::{self, PublicKey, TrustError};
use rugen_proto::{
    IdentityStatement, MatchupView, ResultAck, ResultStatement, SignedBlob, TruthFeed,
    VerdictStatement, purpose,
};

/// Address of the official server (build-time setting).
pub fn server() -> &'static str {
    trust::OFFICIAL_SERVER
}

/// The official server's public key (build-time setting).
pub fn key() -> Option<PublicKey> {
    trust::official_key()
}

/// Same server address (scheme / trailing slash / case insensitive).
pub fn same_server(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        rugen_net::online::base_url(s)
            .trim_end_matches('/')
            .to_ascii_lowercase()
    };
    !a.trim().is_empty() && norm(a) == norm(b)
}

/// Host of an address for list labels (`https://play.rugen.dev` → `play.rugen.dev`).
pub fn host_label(addr: &str) -> &str {
    let rest = addr.split("://").nth(1).unwrap_or(addr);
    rest.trim_end_matches('/')
}

pub fn is_official_addr(addr: &str) -> bool {
    same_server(addr, server())
}

/// What we know about who runs a server.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Trust {
    /// Not probed yet / does not sign (older server).
    #[default]
    Unknown,
    /// Signed our nonce with the official key.
    Official,
    /// Signs with its own key (fingerprint).
    Community(String),
    /// Claimed to be official (or answered for the official address) but
    /// could not prove it.
    Impostor,
}

impl Trust {
    pub fn is_official(&self) -> bool {
        *self == Trust::Official
    }
}

/// A random challenge for `GET /v1/identity` (OS cryptographic RNG).
pub fn nonce() -> String {
    rugen_fair::trust::random_hex(16)
}

/// Judge a server's identity answer to our `nonce`. `claims_official` = it
/// says so in its info or it sits at the official address.
pub fn identity_trust(blob: Option<&SignedBlob>, nonce: &str, claims_official: bool) -> Trust {
    let Some(blob) = blob else {
        return if claims_official {
            Trust::Impostor
        } else {
            Trust::Unknown
        };
    };
    match trust::open::<IdentityStatement>(blob, purpose::IDENTITY, None) {
        Ok((st, k)) if st.nonce == nonce => {
            if Some(k) == key() {
                Trust::Official
            } else if claims_official || st.official {
                Trust::Impostor
            } else {
                Trust::Community(k.fingerprint())
            }
        }
        _ => Trust::Impostor,
    }
}

/// How a verdict / result is stamped on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stamp {
    /// Signed by the official key and matching what is shown.
    Official,
    /// Signed by a community server's own key (fingerprint).
    Community(String),
    /// No signature (older server, or nothing to sign yet).
    Unsigned,
    /// A signature that does not verify or does not match: ignored.
    Forged(String),
}

impl Stamp {
    pub fn label(&self, what: &str) -> String {
        match self {
            Stamp::Official => format!("OFFICIAL {what} (SIGNED)"),
            Stamp::Community(fp) => format!(
                "COMMUNITY {what} · {}",
                fp.get(..9).unwrap_or(fp).to_uppercase()
            ),
            Stamp::Unsigned => format!("UNSIGNED {what}"),
            Stamp::Forged(_) => format!("{what}: BAD SIGNATURE"),
        }
    }

    pub fn is_official(&self) -> bool {
        *self == Stamp::Official
    }
}

fn signer_stamp(k: PublicKey) -> Stamp {
    if Some(k) == key() {
        Stamp::Official
    } else {
        Stamp::Community(k.fingerprint())
    }
}

/// Check the signed verdict of a matchup view against what it shows.
pub fn verdict_stamp(view: &MatchupView) -> Stamp {
    let Some(blob) = &view.signed else {
        return Stamp::Unsigned;
    };
    let Some(verdict) = &view.verdict else {
        return Stamp::Unsigned;
    };
    match trust::open::<VerdictStatement>(blob, purpose::VERDICT, None) {
        Ok((st, k)) => {
            let shown: Vec<&str> = view.fingerprints().iter().flatten().copied().collect();
            let signed: Vec<&str> = st.fingerprints.iter().map(String::as_str).collect();
            if st.match_id != view.id
                || st.verdict != verdict.kind
                || st.reasons != verdict.reasons
                || st.ranked != view.ranked
                || shown.iter().zip(&signed).any(|(a, b)| a != b)
            {
                Stamp::Forged("the signed verdict differs from the one shown".into())
            } else {
                signer_stamp(k)
            }
        }
        Err(e) => Stamp::Forged(e.to_string()),
    }
}

/// Check a result acknowledgement for `match_id` / account `me`.
pub fn result_stamp(ack: &ResultAck, match_id: u64, me: u64) -> Stamp {
    let Some(blob) = &ack.signed else {
        return Stamp::Unsigned;
    };
    match trust::open::<ResultStatement>(blob, purpose::RESULT, None) {
        Ok((st, k)) => {
            if st.match_id != match_id
                || (me != 0 && st.account != me)
                || st.delta != ack.delta.filter(|_| st.ranked)
            {
                Stamp::Forged("the signed result differs from the one shown".into())
            } else {
                signer_stamp(k)
            }
        }
        Err(e) => Stamp::Forged(e.to_string()),
    }
}

// ---- truth feed cache

/// The verified official feed held in memory (loaded from the cache once).
static FEED: Mutex<Option<Option<TruthFeed>>> = Mutex::new(None);
static CACHE: Mutex<Option<PathBuf>> = Mutex::new(None);

fn cache_file(cache_dir: &Path) -> PathBuf {
    cache_dir.join("official_truth.json")
}

/// Verify a cached / downloaded blob with `key` and the freshness rules.
fn open_feed(blob: &SignedBlob, key: &PublicKey, have: u64) -> Result<TruthFeed, TrustError> {
    trust::open_feed(blob, key, trust::now(), have)
}

fn read_cached(cache_dir: &Path) -> Option<SignedBlob> {
    let text = std::fs::read_to_string(cache_file(cache_dir)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Sequence of the cached feed (even a stale one: no rollback below it).
fn cached_sequence(cache_dir: &Path, key: &PublicKey) -> u64 {
    read_cached(cache_dir)
        .and_then(|b| trust::open::<TruthFeed>(&b, purpose::TRUTH, Some(key)).ok())
        .map_or(0, |(f, _)| f.sequence)
}

/// Load the cached feed for this game root (call once at start).
pub fn init(cache_dir: &Path) {
    if let Ok(mut c) = CACHE.lock() {
        *c = Some(cache_dir.to_path_buf());
    }
    if let Ok(mut f) = FEED.lock() {
        *f = None;
    }
}

/// Verify and cache a feed blob downloaded from the official server (or a
/// community mirror: the signature is what counts).
pub fn store_feed(cache_dir: &Path, blob: &SignedBlob) -> Result<TruthFeed, TrustError> {
    let key = key().ok_or(TrustError::BadKey)?;
    let have = cached_sequence(cache_dir, &key);
    let feed = open_feed(blob, &key, have)?;
    let _ = std::fs::create_dir_all(cache_dir);
    if let Err(e) = std::fs::write(
        cache_file(cache_dir),
        serde_json::to_vec(blob).unwrap_or_default(),
    ) {
        log::warn!("official feed cache: {e}");
    }
    if let Ok(mut f) = FEED.lock() {
        *f = Some(Some(feed.clone()));
    }
    Ok(feed)
}

/// The current verified, fresh official feed (from memory or the cache).
pub fn feed() -> Option<TruthFeed> {
    let mut f = FEED.lock().ok()?;
    if f.is_none() {
        let dir = CACHE.lock().ok()?.clone()?;
        let loaded = key().and_then(|k| {
            let blob = read_cached(&dir)?;
            match open_feed(&blob, &k, 0) {
                Ok(feed) => Some(feed),
                Err(e) => {
                    log::info!("cached official feed ignored: {e}");
                    None
                }
            }
        });
        *f = Some(loaded);
    }
    f.as_ref()?
        .as_ref()
        .filter(|x| x.expires_at > trust::now())
        .cloned()
}

/// Why the official server blocks a fingerprint, if it does.
pub fn blocked(fingerprint: &str) -> Option<String> {
    feed()?
        .blocked(fingerprint)
        .map(|b| b.reason.to_uppercase())
}

/// Official community rating of a fingerprint: (rating, games).
pub fn rating(fingerprint: &str) -> Option<(i32, u32)> {
    feed()?.rating(fingerprint).map(|r| (r.rating, r.games))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rugen_fair::trust::SigningKey;
    use rugen_proto::{BlockEntry, TRUST_VERSION, Verdict, VerdictKind};

    // these tests assume the default (development) official key
    fn dev_build() -> bool {
        trust::official_is_dev()
    }

    #[test]
    fn identity_needs_the_official_key_and_our_nonce() {
        if !dev_build() {
            return;
        }
        let n = nonce();
        assert_eq!(n.len(), 32);
        let st = |nonce: &str, official: bool| IdentityStatement {
            v: TRUST_VERSION,
            nonce: nonce.into(),
            name: "x".into(),
            official,
            issued_at: 1,
        };
        let official = SigningKey::dev()
            .sign(purpose::IDENTITY, &st(&n, true))
            .unwrap();
        assert_eq!(identity_trust(Some(&official), &n, true), Trust::Official);
        // replaying the official answer to another nonce
        assert_eq!(
            identity_trust(Some(&official), "00ff", true),
            Trust::Impostor
        );
        let other = SigningKey::generate().unwrap();
        let community = other.sign(purpose::IDENTITY, &st(&n, false)).unwrap();
        assert_eq!(
            identity_trust(Some(&community), &n, false),
            Trust::Community(other.public().fingerprint())
        );
        // a community key claiming to be official, or the official address
        // answering with another key
        let liar = other.sign(purpose::IDENTITY, &st(&n, true)).unwrap();
        assert_eq!(identity_trust(Some(&liar), &n, false), Trust::Impostor);
        assert_eq!(identity_trust(Some(&community), &n, true), Trust::Impostor);
        assert_eq!(identity_trust(None, &n, true), Trust::Impostor);
        assert_eq!(identity_trust(None, &n, false), Trust::Unknown);
    }

    fn view(signer: &SigningKey) -> MatchupView {
        let verdict = Verdict {
            kind: VerdictKind::Blocked,
            reasons: vec!["B: blocked by the official server (regen)".into()],
            ratio: 1.2,
        };
        let st = VerdictStatement {
            v: TRUST_VERSION,
            match_id: 9,
            ranked: false,
            fingerprints: vec![],
            verdict: verdict.kind,
            reasons: verdict.reasons.clone(),
            issued_at: 5,
        };
        MatchupView {
            id: 9,
            verdict: Some(verdict),
            signed: Some(signer.sign(purpose::VERDICT, &st).unwrap()),
            ..Default::default()
        }
    }

    #[test]
    fn verdict_stamps_and_forged_official_verdicts() {
        if !dev_build() {
            return;
        }
        assert_eq!(verdict_stamp(&view(&SigningKey::dev())), Stamp::Official);
        let other = SigningKey::generate().unwrap();
        assert_eq!(
            verdict_stamp(&view(&other)),
            Stamp::Community(other.public().fingerprint())
        );
        // a community server pasting the official key on its own signature
        let mut forged = view(&other);
        if let Some(b) = forged.signed.as_mut() {
            b.key = SigningKey::dev().public().hex();
        }
        assert!(matches!(verdict_stamp(&forged), Stamp::Forged(_)));
        // a genuine official signature reused for a different verdict
        let mut swapped = view(&SigningKey::dev());
        if let Some(v) = swapped.verdict.as_mut() {
            v.kind = VerdictKind::Fair;
        }
        assert!(matches!(verdict_stamp(&swapped), Stamp::Forged(_)));
        let mut unsigned = view(&SigningKey::dev());
        unsigned.signed = None;
        assert_eq!(verdict_stamp(&unsigned), Stamp::Unsigned);
    }

    #[test]
    fn result_stamp_checks_match_account_and_delta() {
        if !dev_build() {
            return;
        }
        let st = ResultStatement {
            v: TRUST_VERSION,
            match_id: 3,
            ranked: true,
            account: 7,
            players: [7, 8],
            winner: 7,
            rating: Some(1216),
            delta: Some(16),
            season: 1,
            issued_at: 1,
        };
        let ack = |signer: &SigningKey, delta: i32| ResultAck {
            accepted: true,
            rating: Some(1216),
            delta: Some(delta),
            disputed: false,
            signed: Some(signer.sign(purpose::RESULT, &st).unwrap()),
        };
        assert_eq!(
            result_stamp(&ack(&SigningKey::dev(), 16), 3, 7),
            Stamp::Official
        );
        assert!(matches!(
            result_stamp(&ack(&SigningKey::dev(), 99), 3, 7),
            Stamp::Forged(_)
        ));
        assert!(matches!(
            result_stamp(&ack(&SigningKey::dev(), 16), 4, 7),
            Stamp::Forged(_)
        ));
        let other = SigningKey::generate().unwrap();
        assert!(matches!(
            result_stamp(&ack(&other, 16), 3, 7),
            Stamp::Community(_)
        ));
    }

    #[test]
    fn feed_cache_verifies_and_refuses_rollback_and_forgeries() {
        if !dev_build() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("rugen-official-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let now = trust::now();
        let feed = |seq: u64| TruthFeed {
            v: TRUST_VERSION,
            server: "official".into(),
            issued_at: now,
            expires_at: now + trust::FEED_VALIDITY,
            sequence: seq,
            blocked: vec![BlockEntry {
                fingerprint: "e".repeat(64),
                reason: "regen".into(),
                since: now,
                source: "admin".into(),
            }],
            ratings: vec![],
        };
        let dev = SigningKey::dev();
        store_feed(&dir, &dev.sign(purpose::TRUTH, &feed(10)).unwrap()).unwrap();
        init(&dir);
        assert_eq!(blocked(&"e".repeat(64)).as_deref(), Some("REGEN"));
        // older copy, other signer: refused, the cache keeps the good one
        assert!(matches!(
            store_feed(&dir, &dev.sign(purpose::TRUTH, &feed(9)).unwrap()),
            Err(TrustError::Rollback { .. })
        ));
        let rogue = SigningKey::generate().unwrap();
        assert!(store_feed(&dir, &rogue.sign(purpose::TRUTH, &feed(11)).unwrap()).is_err());
        init(&dir);
        assert!(blocked(&"e".repeat(64)).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
