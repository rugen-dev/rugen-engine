//! HTTP: the release JSON, `SHA256SUMS`, its signature and the archive
//! download (ureq, rustls). Every call has timeouts; downloads stream to a
//! file with a progress callback and stop at [`crate::MAX_DOWNLOAD`].
//!
//! Every address is checked against the [`Sources`] before it is asked,
//! and redirects are followed here, one at a time, so that each address
//! a redirect points to is checked too.

use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::Context;

use crate::{Release, Sources};

/// Redirects followed for one request.
const MAX_REDIRECTS: usize = 5;

fn agent(global: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_global(Some(global))
        .http_status_as_error(false)
        // redirects are followed by `get`, which checks each address
        .max_redirects(0)
        .user_agent(concat!("rugen/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// `GET url`, following redirects by hand: `sources` is asked about `url`
/// and about every address a redirect names. The response is the first
/// that is not a redirect.
fn get(
    global: Duration,
    url: &str,
    accept: Option<&str>,
    sources: &Sources,
) -> anyhow::Result<ureq::http::Response<ureq::Body>> {
    let agent = agent(global);
    let mut url = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        sources.check(&url)?;
        let mut req = agent.get(&url);
        if let Some(a) = accept {
            req = req.header("Accept", a);
        }
        let resp = req.call().with_context(|| format!("GET {url}"))?;
        if !matches!(resp.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            return Ok(resp);
        }
        let to = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| anyhow::anyhow!("GET {url}: redirect without an address"))?;
        // GitHub redirects to whole addresses; a relative one is not followed
        anyhow::ensure!(
            to.starts_with("https://") || to.starts_with("http://"),
            "GET {url}: redirect to {to:?} not followed"
        );
        url = to.to_string();
    }
    anyhow::bail!("GET {url}: too many redirects")
}

/// `GET url` as text (the release JSON, `SHA256SUMS`); at most `limit` bytes.
pub fn fetch_text(url: &str, limit: u64, sources: &Sources) -> anyhow::Result<String> {
    let mut resp = get(
        Duration::from_secs(30),
        url,
        Some("application/vnd.github+json, application/json, text/plain"),
        sources,
    )?;
    let status = resp.status().as_u16();
    anyhow::ensure!(status == 200, "GET {url}: http {status}");
    resp.body_mut()
        .with_config()
        .limit(limit)
        .read_to_string()
        .with_context(|| format!("GET {url}: body"))
}

/// The latest release from `url` (GitHub's `releases/latest` JSON).
pub fn fetch_latest(url: &str, sources: &Sources) -> anyhow::Result<Release> {
    let text = fetch_text(url, 4 * 1024 * 1024, sources)?;
    let v: serde_json::Value = serde_json::from_str(&text).context("release json")?;
    Release::from_github(&v)
}

/// Stream `url` into `dest`, calling `progress(received, total)` as bytes
/// arrive. Fails past `max` bytes (and removes the partial file). Returns
/// the SHA-256 of what was written.
pub fn download(
    url: &str,
    dest: &Path,
    max: u64,
    sources: &Sources,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> anyhow::Result<String> {
    use sha2::Digest;
    let mut resp = get(Duration::from_secs(20 * 60), url, None, sources)?;
    let status = resp.status().as_u16();
    anyhow::ensure!(status == 200, "GET {url}: http {status}");
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    if let Some(t) = total {
        anyhow::ensure!(t <= max, "{url}: {t} bytes, more than the {max} byte limit");
    }
    let result = (|| -> anyhow::Result<String> {
        let mut file =
            std::fs::File::create(dest).with_context(|| format!("create {}", dest.display()))?;
        let mut reader = resp.body_mut().with_config().limit(max).reader();
        let mut hash = sha2::Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut got = 0u64;
        progress(0, total);
        loop {
            let n = reader.read(&mut buf).context("read body")?;
            if n == 0 {
                break;
            }
            got += n as u64;
            anyhow::ensure!(got <= max, "{url}: more than the {max} byte limit");
            file.write_all(&buf[..n])
                .with_context(|| format!("write {}", dest.display()))?;
            hash.update(&buf[..n]);
            progress(got, total);
        }
        if let Some(t) = total {
            anyhow::ensure!(got == t, "{url}: got {got} of {t} bytes");
        }
        file.flush()?;
        Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    result
}
