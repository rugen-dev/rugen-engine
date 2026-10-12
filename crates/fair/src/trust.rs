//! Signatures of the official server (and of community servers that sign
//! with their own key): Ed25519 over a canonical JSON payload.
//!
//! # Signed message
//!
//! A [`SignedBlob`] carries `purpose`, `payload`, `key` and `sig`. The
//! signature covers the UTF-8 bytes of
//!
//! ```text
//! rugen-signed-v1 \n <purpose> \n <payload>        (no spaces around \n)
//! ```
//!
//! so a signature made for one purpose (`truth`, `verdict`, `result`,
//! `identity`) never verifies as another.
//!
//! # Canonical JSON
//!
//! The payload is the statement serialised as *canonical JSON*:
//!
//! * no whitespace at all (`{"a":1,"b":[true,null]}`);
//! * object keys sorted by their UTF-8 bytes, each key once;
//! * numbers are integers only, written in decimal without sign for
//!   non-negative values, exponent or leading zeros (floats are refused);
//! * strings as serde_json writes them: `"` `\` and control characters
//!   escaped (`\n \r \t \b \f`, others `\u00XX`), everything else verbatim
//!   UTF-8 (no `\/`, no escaping of non-ASCII).
//!
//! Verification checks the signature over the received bytes *and* that the
//! payload is canonical (parse + re-encode is byte-identical), so there is
//! exactly one valid encoding of a statement.
//!
//! # Keys
//!
//! Keys are 32-byte Ed25519 seeds (private) and public keys, written as 64
//! hex digits (base64 is accepted when parsing). The official public key
//! and address are baked into the build ([`OFFICIAL_KEY`],
//! [`OFFICIAL_SERVER`]) from the `RUGEN_OFFICIAL_KEY` /
//! `RUGEN_OFFICIAL_SERVER` environment variables at compile time; without
//! them the defaults below are used: the official domain `play.rugen.dev`
//! and the development key, whose private half is public (see
//! [`DEV_SIGNING_SEED`]): until a release is built with the real public
//! key, nothing such a build shows as OFFICIAL is trustworthy.

use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use rugen_proto::{SignedBlob, TRUST_VERSION, TruthFeed, from_hex, to_hex};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Official address of builds made without `RUGEN_OFFICIAL_SERVER`.
pub const DEFAULT_OFFICIAL_SERVER: &str = "https://play.rugen.dev";

/// Address of the official rugen server (pinned first in the ONLINE list,
/// default feed source of community servers). Build-time setting.
pub const OFFICIAL_SERVER: &str = match option_env!("RUGEN_OFFICIAL_SERVER") {
    Some(s) if !s.is_empty() => s,
    _ => DEFAULT_OFFICIAL_SERVER,
};

/// Seed of the development key: **public knowledge**, for tests and local
/// servers only. A release must be built with a real `RUGEN_OFFICIAL_KEY`.
pub const DEV_SIGNING_SEED: &str =
    "b8d870228db96f43b2a54bde0652291b930ce51838ae6d0c4761bbed6e439a4d";

/// Public half of [`DEV_SIGNING_SEED`] (the placeholder official key).
pub const DEV_PUBLIC_KEY: &str = "b87d6c95e5062a38417566e3637ddfa93202ec7c416b42820f81baed988f627b";

/// The official server's Ed25519 public key (hex). Build-time setting.
pub const OFFICIAL_KEY: &str = match option_env!("RUGEN_OFFICIAL_KEY") {
    Some(s) if !s.is_empty() => s,
    _ => DEV_PUBLIC_KEY,
};

/// Domain prefix of every signed message.
pub const SIG_DOMAIN: &str = "rugen-signed-v1";

/// A truth feed must expire at most this long after it was issued.
pub const FEED_MAX_VALIDITY: u64 = 7 * 24 * 3600;
/// Validity the official server gives a feed it signs.
pub const FEED_VALIDITY: u64 = 3 * 24 * 3600;
/// Tolerated clock difference for "issued in the future".
pub const CLOCK_SKEW: u64 = 10 * 60;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TrustError {
    #[error("bad key")]
    BadKey,
    #[error("unsigned or malformed signature")]
    BadSignature,
    #[error("signature does not verify")]
    Forged,
    #[error("signed by {got}, expected {want}")]
    WrongKey { got: String, want: String },
    #[error("signed for {got:?}, expected {want:?}")]
    WrongPurpose { got: String, want: String },
    #[error("payload is not canonical JSON")]
    NotCanonical,
    #[error("floats are not allowed in signed payloads")]
    Float,
    #[error("payload: {0}")]
    Payload(String),
    #[error("unsupported statement version {0}")]
    Version(u32),
    #[error("stale: expired {0} s ago")]
    Stale(u64),
    #[error("validity too long")]
    TooLong,
    #[error("issued in the future")]
    Future,
    #[error("older than the feed already held (sequence {got} < {have})")]
    Rollback { got: u64, have: u64 },
    #[error("random generator failed")]
    Random,
}

/// Parse 32 bytes from 64 hex digits or (padded / unpadded) base64.
fn parse32(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    let bytes = if s.len() == 64 {
        from_hex(&s.to_ascii_lowercase())?
    } else {
        rugen_proto::b64_decode(s)?
    };
    bytes.try_into().ok()
}

/// An Ed25519 public key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PublicKey([u8; 32]);

impl PublicKey {
    pub fn parse(s: &str) -> Option<Self> {
        parse32(s).map(Self)
    }

    pub fn from_bytes(b: [u8; 32]) -> Self {
        Self(b)
    }

    pub fn hex(&self) -> String {
        to_hex(&self.0)
    }

    /// Short human form for cards: first 8 bytes of sha256(key), `ab12:cd34:..`.
    pub fn fingerprint(&self) -> String {
        let h = crate::sha256_hex(&self.0);
        h.as_bytes()
            .chunks(4)
            .take(4)
            .map(|c| std::str::from_utf8(c).unwrap_or(""))
            .collect::<Vec<_>>()
            .join(":")
    }

    /// Verify `sig` (64 bytes) over `msg`.
    fn verify_raw(&self, msg: &[u8], sig: &[u8]) -> bool {
        UnparsedPublicKey::new(&ED25519, &self.0)
            .verify(msg, sig)
            .is_ok()
    }
}

impl std::fmt::Display for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.fingerprint())
    }
}

/// The official public key of this build (`None` only if the baked value is malformed).
pub fn official_key() -> Option<PublicKey> {
    PublicKey::parse(OFFICIAL_KEY)
}

/// This build trusts the public development key: nothing it shows as
/// OFFICIAL is really official (local testing builds).
pub fn official_is_dev() -> bool {
    OFFICIAL_KEY.eq_ignore_ascii_case(DEV_PUBLIC_KEY)
}

/// The official address is the default one (`RUGEN_OFFICIAL_SERVER` unset).
pub fn official_is_default() -> bool {
    OFFICIAL_SERVER == DEFAULT_OFFICIAL_SERVER
}

/// An Ed25519 signing key (a server's private key).
pub struct SigningKey {
    seed: [u8; 32],
    pair: Ed25519KeyPair,
}

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SigningKey({})", self.public())
    }
}

impl Clone for SigningKey {
    fn clone(&self) -> Self {
        Self::from_seed(self.seed)
    }
}

impl SigningKey {
    pub fn from_seed(seed: [u8; 32]) -> Self {
        let pair = Ed25519KeyPair::from_seed_unchecked(&seed).expect("any 32 bytes are a seed");
        Self { seed, pair }
    }

    /// From 64 hex digits or base64 of the 32-byte seed.
    pub fn parse(s: &str) -> Option<Self> {
        parse32(s).map(Self::from_seed)
    }

    /// A fresh random key (operating system randomness).
    pub fn generate() -> Result<Self, TrustError> {
        use ring::rand::SecureRandom as _;
        let mut seed = [0u8; 32];
        ring::rand::SystemRandom::new()
            .fill(&mut seed)
            .map_err(|_| TrustError::Random)?;
        Ok(Self::from_seed(seed))
    }

    /// The public development key's private half (tests, local servers).
    pub fn dev() -> Self {
        Self::parse(DEV_SIGNING_SEED).expect("dev seed")
    }

    /// Seed as 64 hex digits (keep secret).
    pub fn seed_hex(&self) -> String {
        to_hex(&self.seed)
    }

    pub fn public(&self) -> PublicKey {
        let mut b = [0u8; 32];
        b.copy_from_slice(self.pair.public_key().as_ref());
        PublicKey(b)
    }

    /// Sign `statement` (canonical JSON) for `purpose`.
    pub fn sign<T: Serialize>(
        &self,
        purpose: &str,
        statement: &T,
    ) -> Result<SignedBlob, TrustError> {
        let payload = canonicalize(statement)?;
        let sig = self.pair.sign(&message(purpose, &payload));
        Ok(SignedBlob {
            purpose: purpose.to_string(),
            payload,
            key: self.public().hex(),
            sig: to_hex(sig.as_ref()),
        })
    }
}

/// The exact bytes a signature covers.
pub fn message(purpose: &str, payload: &str) -> Vec<u8> {
    let mut m = Vec::with_capacity(SIG_DOMAIN.len() + purpose.len() + payload.len() + 2);
    m.extend_from_slice(SIG_DOMAIN.as_bytes());
    m.push(b'\n');
    m.extend_from_slice(purpose.as_bytes());
    m.push(b'\n');
    m.extend_from_slice(payload.as_bytes());
    m
}

/// Canonical JSON text of a JSON value (see the module docs).
pub fn canonical(v: &Value) -> Result<String, TrustError> {
    fn write(v: &Value, out: &mut String) -> Result<(), TrustError> {
        match v {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => {
                if n.is_i64() || n.is_u64() {
                    out.push_str(&n.to_string());
                } else {
                    return Err(TrustError::Float);
                }
            }
            Value::String(s) => out.push_str(
                &serde_json::to_string(s).map_err(|e| TrustError::Payload(e.to_string()))?,
            ),
            Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write(x, out)?;
                }
                out.push(']');
            }
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
                out.push('{');
                for (i, k) in keys.into_iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(
                        &serde_json::to_string(k)
                            .map_err(|e| TrustError::Payload(e.to_string()))?,
                    );
                    out.push(':');
                    write(&m[k], out)?;
                }
                out.push('}');
            }
        }
        Ok(())
    }
    let mut out = String::new();
    write(v, &mut out)?;
    Ok(out)
}

/// Canonical JSON of any serialisable statement.
pub fn canonicalize<T: Serialize>(t: &T) -> Result<String, TrustError> {
    let v = serde_json::to_value(t).map_err(|e| TrustError::Payload(e.to_string()))?;
    canonical(&v)
}

/// Check a blob: purpose, signer (`expect` = this key only), signature and
/// canonical payload. Returns the signer.
pub fn verify(
    blob: &SignedBlob,
    purpose: &str,
    expect: Option<&PublicKey>,
) -> Result<PublicKey, TrustError> {
    if blob.purpose != purpose {
        return Err(TrustError::WrongPurpose {
            got: blob.purpose.clone(),
            want: purpose.into(),
        });
    }
    let key = PublicKey::parse(&blob.key).ok_or(TrustError::BadKey)?;
    if let Some(want) = expect
        && *want != key
    {
        return Err(TrustError::WrongKey {
            got: key.fingerprint(),
            want: want.fingerprint(),
        });
    }
    let sig = match from_hex(&blob.sig) {
        Some(s) if s.len() == 64 => s,
        _ => return Err(TrustError::BadSignature),
    };
    if !key.verify_raw(&message(purpose, &blob.payload), &sig) {
        return Err(TrustError::Forged);
    }
    let v: Value =
        serde_json::from_str(&blob.payload).map_err(|e| TrustError::Payload(e.to_string()))?;
    if canonical(&v)? != blob.payload {
        return Err(TrustError::NotCanonical);
    }
    Ok(key)
}

/// [`verify`], then decode the statement.
pub fn open<T: DeserializeOwned>(
    blob: &SignedBlob,
    purpose: &str,
    expect: Option<&PublicKey>,
) -> Result<(T, PublicKey), TrustError> {
    let key = verify(blob, purpose, expect)?;
    let t = serde_json::from_str(&blob.payload).map_err(|e| TrustError::Payload(e.to_string()))?;
    Ok((t, key))
}

/// Freshness of a verified feed at `now`; `have` = sequence already held.
pub fn check_feed(feed: &TruthFeed, now: u64, have: u64) -> Result<(), TrustError> {
    if feed.v != TRUST_VERSION {
        return Err(TrustError::Version(feed.v));
    }
    if feed.issued_at > now + CLOCK_SKEW {
        return Err(TrustError::Future);
    }
    if feed.expires_at <= now {
        return Err(TrustError::Stale(now - feed.expires_at));
    }
    if feed.expires_at > feed.issued_at + FEED_MAX_VALIDITY {
        return Err(TrustError::TooLong);
    }
    if feed.sequence < have {
        return Err(TrustError::Rollback {
            got: feed.sequence,
            have,
        });
    }
    Ok(())
}

/// Verify a feed blob signed by `key` and check its freshness.
pub fn open_feed(
    blob: &SignedBlob,
    key: &PublicKey,
    now: u64,
    have: u64,
) -> Result<TruthFeed, TrustError> {
    let (feed, _) = open::<TruthFeed>(blob, rugen_proto::purpose::TRUTH, Some(key))?;
    check_feed(&feed, now, have)?;
    Ok(feed)
}

/// Unix seconds now.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rugen_proto::{BlockEntry, VerdictKind, VerdictStatement, purpose};
    use serde_json::json;

    fn feed(now: u64, seq: u64) -> TruthFeed {
        TruthFeed {
            v: TRUST_VERSION,
            server: "official".into(),
            issued_at: now,
            expires_at: now + FEED_VALIDITY,
            sequence: seq,
            blocked: vec![BlockEntry {
                fingerprint: "a".repeat(64),
                reason: "regen \"cheat\"\n".into(),
                since: 1,
                source: "admin".into(),
            }],
            ratings: vec![],
        }
    }

    #[test]
    fn dev_key_matches_its_public_half() {
        assert_eq!(SigningKey::dev().public().hex(), DEV_PUBLIC_KEY);
        assert!(official_key().is_some(), "baked official key parses");
        let k = SigningKey::generate().unwrap();
        assert_eq!(
            SigningKey::parse(&k.seed_hex()).unwrap().public(),
            k.public()
        );
        let b64 = rugen_proto::b64_encode(&k.public().0);
        assert_eq!(PublicKey::parse(&b64), Some(k.public()));
        assert_eq!(k.public().fingerprint().len(), 19);
    }

    #[test]
    fn canonical_form_is_sorted_compact_and_integer_only() {
        let v = json!({"b": [1, true, null], "a": {"z": "x\"y\n", "é": -3}, "A": u64::MAX});
        assert_eq!(
            canonical(&v).unwrap(),
            r#"{"A":18446744073709551615,"a":{"z":"x\"y\n","é":-3},"b":[1,true,null]}"#
        );
        assert_eq!(canonical(&json!({"f": 1.5})), Err(TrustError::Float));
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let k = SigningKey::dev();
        let st = VerdictStatement {
            v: TRUST_VERSION,
            match_id: 7,
            ranked: true,
            fingerprints: vec!["a".repeat(64), "b".repeat(64)],
            verdict: VerdictKind::Blocked,
            reasons: vec!["X: blocked by the official server".into()],
            issued_at: 100,
        };
        let blob = k.sign(purpose::VERDICT, &st).unwrap();
        let (back, signer) =
            open::<VerdictStatement>(&blob, purpose::VERDICT, Some(&k.public())).unwrap();
        assert_eq!(back, st);
        assert_eq!(signer, k.public());
        // any signer accepted when no key is expected
        assert!(verify(&blob, purpose::VERDICT, None).is_ok());
        // wrong purpose: a verdict is not a feed
        assert!(matches!(
            verify(&blob, purpose::TRUTH, None),
            Err(TrustError::WrongPurpose { .. })
        ));
    }

    #[test]
    fn forged_official_statements_are_rejected() {
        let official = SigningKey::dev();
        let rogue = SigningKey::generate().unwrap();
        let st = feed(1000, 1);
        // signed by another key but claiming to be official
        let mut forged = rogue.sign(purpose::TRUTH, &st).unwrap();
        assert!(matches!(
            verify(&forged, purpose::TRUTH, Some(&official.public())),
            Err(TrustError::WrongKey { .. })
        ));
        // the official key pasted on a rogue signature
        forged.key = official.public().hex();
        assert_eq!(
            verify(&forged, purpose::TRUTH, Some(&official.public())),
            Err(TrustError::Forged)
        );
        // a genuine blob with an edited payload (blocked entry removed)
        let mut edited = official.sign(purpose::TRUTH, &st).unwrap();
        let mut f = st.clone();
        f.blocked.clear();
        edited.payload = canonicalize(&f).unwrap();
        assert_eq!(
            verify(&edited, purpose::TRUTH, Some(&official.public())),
            Err(TrustError::Forged)
        );
        // unsigned / garbage
        let mut unsigned = official.sign(purpose::TRUTH, &st).unwrap();
        unsigned.sig.clear();
        assert_eq!(
            verify(&unsigned, purpose::TRUTH, None),
            Err(TrustError::BadSignature)
        );
        // same statement, non-canonical encoding (whitespace): refused
        let pretty = serde_json::to_string_pretty(&st).unwrap();
        let sig = official.pair.sign(&message(purpose::TRUTH, &pretty));
        let loose = SignedBlob {
            purpose: purpose::TRUTH.into(),
            payload: pretty,
            key: official.public().hex(),
            sig: to_hex(sig.as_ref()),
        };
        assert_eq!(
            verify(&loose, purpose::TRUTH, None),
            Err(TrustError::NotCanonical)
        );
    }

    #[test]
    fn feed_staleness_and_rollback() {
        let k = SigningKey::dev();
        let now = 1_000_000;
        let blob = k.sign(purpose::TRUTH, &feed(now, 5)).unwrap();
        assert!(open_feed(&blob, &k.public(), now + 60, 5).is_ok());
        // expired
        assert!(matches!(
            open_feed(&blob, &k.public(), now + FEED_VALIDITY + 1, 0),
            Err(TrustError::Stale(_))
        ));
        // older than what we hold
        assert!(matches!(
            open_feed(&blob, &k.public(), now, 6),
            Err(TrustError::Rollback { got: 5, have: 6 })
        ));
        // from the future
        assert_eq!(
            check_feed(&feed(now + CLOCK_SKEW + 10, 1), now, 0),
            Err(TrustError::Future)
        );
        // a validity window nobody may grant
        let mut long = feed(now, 1);
        long.expires_at = now + FEED_MAX_VALIDITY + 1;
        assert_eq!(check_feed(&long, now, 0), Err(TrustError::TooLong));
        let mut v2 = feed(now, 1);
        v2.v = 99;
        assert_eq!(check_feed(&v2, now, 0), Err(TrustError::Version(99)));
    }
}
