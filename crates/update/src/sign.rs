//! Who made a release: `SHA256SUMS` is signed, and the game checks the
//! signature against a key baked into it.
//!
//! The checksums alone only say an archive arrived whole. They are
//! published next to the archives, so whoever can publish a release (a
//! stolen token, a compromised build) can publish matching checksums too.
//! The signature is made with a private key that is not on GitHub's side
//! of that door; the game holds the public half and installs nothing the
//! key did not sign.
//!
//! - `SHA256SUMS.sig` ([`SIG_ASSET`]) is the Ed25519 signature, as 128 hex
//!   digits, of [`message`]: a fixed first line, then the bytes of
//!   `SHA256SUMS` exactly as published.
//! - The public key (64 hex digits) is baked in from `RUGEN_UPDATE_KEY` at
//!   compile time ([`UPDATE_KEY`]). A build without it has no key and does
//!   not update itself at all: there is no unsigned fallback.
//! - An archive's name carries its version, and the game only installs the
//!   archive named for the newer version, so an old signed `SHA256SUMS`
//!   cannot pass an old build off as a new one.
//!
//! `rugen-update-key` (this crate's tool) makes a key pair and signs.

use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};

/// The signature asset shipped next to [`crate::SUMS_ASSET`].
pub const SIG_ASSET: &str = "SHA256SUMS.sig";

/// The update public key of this build (hex), from `RUGEN_UPDATE_KEY` at
/// compile time. `None`: this build does not update itself.
pub const UPDATE_KEY: Option<&str> = match option_env!("RUGEN_UPDATE_KEY") {
    Some(s) if !s.is_empty() => Some(s),
    _ => None,
};

/// First line of every signed message: a signature made for something
/// else with the same key never passes for a release.
const DOMAIN: &[u8] = b"rugen-update-v1\n";

/// 32 bytes from 64 hex digits.
pub fn parse_key(s: &str) -> Option<[u8; 32]> {
    unhex(s.trim())?.try_into().ok()
}

/// The baked public key (`None` when unset or malformed).
pub fn update_key() -> Option<[u8; 32]> {
    UPDATE_KEY.and_then(parse_key)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// The exact bytes a signature covers.
pub fn message(sums: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(DOMAIN.len() + sums.len());
    m.extend_from_slice(DOMAIN);
    m.extend_from_slice(sums);
    m
}

/// `sig` (the text of `SHA256SUMS.sig`) is `key`'s signature of `sums`.
pub fn verify(key: &[u8; 32], sums: &[u8], sig: &str) -> anyhow::Result<()> {
    let sig = unhex(sig.trim())
        .filter(|s| s.len() == 64)
        .ok_or_else(|| anyhow::anyhow!("{SIG_ASSET} is not a signature (128 hex digits)"))?;
    UnparsedPublicKey::new(&ED25519, key)
        .verify(&message(sums), &sig)
        .map_err(|_| {
            anyhow::anyhow!(
                "{} is not signed by this build's update key: nothing installed",
                crate::SUMS_ASSET
            )
        })
}

/// A fresh private key (32-byte seed) from the operating system.
pub fn generate() -> anyhow::Result<[u8; 32]> {
    use ring::rand::SecureRandom as _;
    let mut seed = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut seed)
        .map_err(|_| anyhow::anyhow!("random generator failed"))?;
    Ok(seed)
}

/// The public key of a private key.
pub fn public_of(seed: &[u8; 32]) -> anyhow::Result<[u8; 32]> {
    let pair = Ed25519KeyPair::from_seed_unchecked(seed).map_err(|_| anyhow::anyhow!("bad key"))?;
    pair.public_key()
        .as_ref()
        .try_into()
        .map_err(|_| anyhow::anyhow!("bad key"))
}

/// The text of `SHA256SUMS.sig` for `sums`, signed with `seed`.
pub fn sign(seed: &[u8; 32], sums: &[u8]) -> anyhow::Result<String> {
    let pair = Ed25519KeyPair::from_seed_unchecked(seed).map_err(|_| anyhow::anyhow!("bad key"))?;
    Ok(hex(pair.sign(&message(sums)).as_ref()))
}

// --------------------------------------------------------------- addresses

/// Where the updater may fetch from. Every address is checked before it is
/// asked, and so is every address a redirect points to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sources {
    /// One more origin (`scheme://host[:port]`) besides GitHub: the one
    /// `RUGEN_UPDATE_URL` names. Plain http is only kept for this machine.
    extra: Option<String>,
}

/// `scheme://host[:port]` of an absolute http(s) URL, lower case.
fn origin(url: &str) -> Option<(String, String, String)> {
    let uri: ureq::http::Uri = url.parse().ok()?;
    let scheme = uri.scheme_str()?.to_ascii_lowercase();
    let host = uri.host()?.to_ascii_lowercase();
    // `user@host` addresses are refused: what is shown is not what is asked
    if uri.authority()?.as_str().contains('@') {
        return None;
    }
    let port = uri.port_u16().map(|p| p.to_string()).unwrap_or_default();
    Some((scheme, host, port))
}

fn loopback(host: &str) -> bool {
    host == "localhost"
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// GitHub's API, site, and the hosts its release downloads redirect to.
fn github(host: &str) -> bool {
    host == "github.com" || host == "api.github.com" || host.ends_with(".githubusercontent.com")
}

impl Sources {
    /// GitHub only.
    pub fn github() -> Self {
        Self::default()
    }

    /// GitHub, and the origin of `url` when it is not GitHub's (the
    /// `RUGEN_UPDATE_URL` override: a test server, a mirror). An http
    /// origin is kept only when it is this machine.
    pub fn for_release_url(url: &str) -> Self {
        let extra = origin(url)
            .filter(|(scheme, host, _)| {
                !(scheme == "https" && github(host))
                    && (scheme == "https" || (scheme == "http" && loopback(host)))
            })
            .map(|(s, h, p)| format!("{s}://{h}:{p}"));
        Self { extra }
    }

    /// `url` may be fetched: https on a GitHub host, or the extra origin.
    pub fn check(&self, url: &str) -> anyhow::Result<()> {
        let Some((scheme, host, port)) = origin(url) else {
            anyhow::bail!("{url}: not an address the updater fetches from");
        };
        if scheme == "https" && github(&host) && (port.is_empty() || port == "443") {
            return Ok(());
        }
        if self.extra.as_deref() == Some(format!("{scheme}://{host}:{port}").as_str()) {
            return Ok(());
        }
        anyhow::bail!("{url}: the updater only fetches from GitHub over https")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUMS: &[u8] = b"aa  rugen-0.8.0-linux-x86_64.tar.gz\n";

    #[test]
    fn a_good_signature_is_accepted() {
        let seed = [7u8; 32];
        let key = public_of(&seed).unwrap();
        let sig = sign(&seed, SUMS).unwrap();
        assert_eq!(sig.len(), 128);
        verify(&key, SUMS, &sig).unwrap();
        // as a file: a trailing newline does not matter
        verify(&key, SUMS, &format!("{sig}\n")).unwrap();
    }

    #[test]
    fn another_key_changed_sums_or_no_signature_are_refused() {
        let seed = [7u8; 32];
        let key = public_of(&seed).unwrap();
        let sig = sign(&seed, SUMS).unwrap();
        let other = public_of(&[8u8; 32]).unwrap();
        assert!(verify(&other, SUMS, &sig).is_err(), "another key");
        let mut changed = SUMS.to_vec();
        changed[0] = b'b';
        assert!(verify(&key, &changed, &sig).is_err(), "changed sums");
        assert!(verify(&key, SUMS, "").is_err(), "empty");
        assert!(verify(&key, SUMS, "not hex").is_err(), "not hex");
        assert!(verify(&key, SUMS, &sig[..126]).is_err(), "short");
        let zero = "0".repeat(128);
        assert!(verify(&key, SUMS, &zero).is_err(), "zeros");
        // a signature of the same bytes without the first line is not one
        let pair = Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
        let bare = hex(pair.sign(SUMS).as_ref());
        assert!(verify(&key, SUMS, &bare).is_err(), "no domain line");
    }

    #[test]
    fn keys_parse_from_64_hex_digits_only() {
        let key = public_of(&[7u8; 32]).unwrap();
        assert_eq!(parse_key(&hex(&key)), Some(key));
        assert_eq!(parse_key(&format!(" {}\n", hex(&key))), Some(key));
        assert_eq!(parse_key("abcd"), None);
        assert_eq!(parse_key(&"zz".repeat(32)), None);
        assert_eq!(parse_key(""), None);
    }

    #[test]
    fn only_github_over_https_is_fetched() {
        let s = Sources::github();
        for ok in [
            "https://api.github.com/repos/rugen-dev/rugen-engine/releases/latest",
            "https://github.com/rugen-dev/rugen-engine/releases/download/v1/SHA256SUMS",
            "https://objects.githubusercontent.com/x",
            "https://release-assets.githubusercontent.com/x?sig=1",
            "https://GitHub.com/x",
        ] {
            s.check(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for bad in [
            "http://github.com/x",
            "https://evil.example/x",
            "https://github.com.evil.example/x",
            "https://evilgithub.com/x",
            "https://githubusercontent.com.evil.example/x",
            "https://github.com@evil.example/x",
            "https://github.com:8443/x",
            "ftp://github.com/x",
            "file:///etc/passwd",
            "/relative",
            "",
        ] {
            assert!(s.check(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn an_override_adds_its_own_origin_and_nothing_else() {
        let s = Sources::for_release_url("http://127.0.0.1:8123/latest");
        s.check("http://127.0.0.1:8123/rugen.tar.gz").unwrap();
        s.check("https://github.com/x").unwrap();
        assert!(s.check("http://127.0.0.1:9000/x").is_err(), "another port");
        assert!(s.check("http://10.0.0.1:8123/x").is_err(), "another host");
        // plain http is kept for this machine only
        let far = Sources::for_release_url("http://mirror.example/latest");
        assert!(far.check("http://mirror.example/x").is_err());
        let mirror = Sources::for_release_url("https://mirror.example/latest");
        mirror.check("https://mirror.example/x").unwrap();
        assert!(mirror.check("http://mirror.example/x").is_err());
        assert_eq!(
            Sources::for_release_url(crate::LATEST_URL),
            Sources::github()
        );
    }
}
