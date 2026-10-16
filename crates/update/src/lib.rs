//! In-game updater. The source of truth is the GitHub releases of
//! `rugen-dev/rugen-engine`: one archive per platform plus a `SHA256SUMS`
//! asset. This crate holds the pure parts (version compare, asset naming,
//! checksum parsing, install layout detection, the check cache), the
//! network calls ([`net`]), the extract / apply steps ([`apply`]) and the
//! background state machine the menus and `rugen update` drive
//! ([`Updater`]).
//!
//! Nothing here touches player files: `rugen.toml`, `chars/`, `stages/`,
//! `themes/`, `cache/`, `replays/` stay as they are. An update replaces the
//! binary, `motif/` and `data/` of a portable install only; system packages
//! (.deb / .rpm) point the player at the new package instead.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub mod apply;
pub mod net;
mod updater;

pub use updater::{Phase, Snapshot, Updater};

/// The GitHub repository releases come from.
pub const REPO: &str = "rugen-dev/rugen-engine";
/// `releases/latest` of [`REPO`].
pub const LATEST_URL: &str = "https://api.github.com/repos/rugen-dev/rugen-engine/releases/latest";
/// Where the release notes live for players.
pub const NOTES_URL: &str = "https://github.com/rugen-dev/rugen-engine/releases";
/// Largest archive we accept (bytes).
pub const MAX_DOWNLOAD: u64 = 500 * 1024 * 1024;
/// A check per launch at most this often.
pub const CHECK_EVERY_SECS: u64 = 6 * 60 * 60;
/// Checksum asset shipped next to the archives.
pub const SUMS_ASSET: &str = "SHA256SUMS";

/// The `releases/latest` URL: `RUGEN_UPDATE_URL` overrides it (tests, a
/// local file server).
pub fn latest_url() -> String {
    std::env::var("RUGEN_UPDATE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| LATEST_URL.to_string())
}

// ---------------------------------------------------------------- versions

/// `MAJOR.MINOR.PATCH[-pre]`, with or without a leading `v`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// Pre-release tag; a release without one is newer than the same
    /// numbers with one (`0.8.0 > 0.8.0-rc1`).
    pub pre: Option<String>,
}

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        let (nums, pre) = match s.split_once('-') {
            Some((n, p)) => (n, Some(p.to_string())),
            None => (s, None),
        };
        let mut it = nums.split('.');
        let major = it.next()?.parse().ok()?;
        let minor = it.next()?.parse().ok()?;
        let patch = it.next().unwrap_or("0").parse().ok()?;
        if it.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }

    /// Strictly newer than `other`.
    pub fn newer_than(&self, other: &Self) -> bool {
        let a = (self.major, self.minor, self.patch);
        let b = (other.major, other.minor, other.patch);
        if a != b {
            return a > b;
        }
        match (&self.pre, &other.pre) {
            (None, Some(_)) => true,
            (Some(_), None) => false,
            (Some(x), Some(y)) => x > y,
            (None, None) => false,
        }
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(p) = &self.pre {
            write!(f, "-{p}")?;
        }
        Ok(())
    }
}

/// `latest` is a newer release than the running `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (Version::parse(latest), Version::parse(current)) {
        (Some(l), Some(c)) => l.newer_than(&c),
        _ => false,
    }
}

// ---------------------------------------------------------------- assets

/// Archive kinds the updater can unpack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    TarGz,
}

impl ArchiveKind {
    pub fn of(name: &str) -> Option<Self> {
        if name.ends_with(".zip") {
            Some(Self::Zip)
        } else if name.ends_with(".tar.gz") {
            Some(Self::TarGz)
        } else {
            None
        }
    }
}

/// The portable archive the release ships for this `os` / `arch`
/// (`std::env::consts` names), e.g. `rugen-0.8.0-windows-x64.zip`.
pub fn asset_name(version: &str, os: &str, arch: &str) -> Option<String> {
    let v = version.trim_start_matches('v');
    match os {
        "windows" => {
            let a = match arch {
                "x86_64" => "x64",
                "x86" => "x86",
                "aarch64" => "arm64",
                _ => return None,
            };
            Some(format!("rugen-{v}-windows-{a}.zip"))
        }
        "linux" => match arch {
            "x86_64" => Some(format!("rugen-{v}-linux-x86_64.tar.gz")),
            _ => None,
        },
        "macos" => {
            let a = match arch {
                "aarch64" => "arm64",
                "x86_64" => "x64",
                _ => return None,
            };
            Some(format!("rugen-{v}-macos-{a}.tar.gz"))
        }
        _ => None,
    }
}

/// The archive for the machine this binary runs on.
pub fn own_asset_name(version: &str) -> Option<String> {
    asset_name(version, std::env::consts::OS, std::env::consts::ARCH)
}

/// Name of the binary inside the archive and the install.
pub fn exe_name() -> &'static str {
    if cfg!(windows) { "rugen.exe" } else { "rugen" }
}

/// `SHA256SUMS` lines (`<hex>  <name>`, the `sha256sum` format) as
/// `(name, hex)` pairs; blank lines and comments are skipped.
pub fn parse_sha256sums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') {
                return None;
            }
            let (hex, name) = l.split_once(char::is_whitespace)?;
            let name = name.trim().trim_start_matches('*');
            let hex = hex.trim().to_ascii_lowercase();
            (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) && !name.is_empty())
                .then(|| (name.to_string(), hex))
        })
        .collect()
}

/// The expected checksum of `name` in `SHA256SUMS`.
pub fn expected_sha(sums: &str, name: &str) -> Option<String> {
    parse_sha256sums(sums)
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, h)| h)
}

/// Lower-case hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let d = sha2::Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Lower-case hex SHA-256 of a file, streamed.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::Digest;
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

// ---------------------------------------------------------------- releases

/// One downloadable file of a release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// A published release, as much of it as the menus show.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// `0.8.0` (no `v`).
    pub version: String,
    /// `v0.8.0`.
    pub tag: String,
    /// `YYYY-MM-DD` of `published_at`.
    pub date: String,
    /// The release page.
    pub url: String,
    pub assets: Vec<Asset>,
}

impl Release {
    /// From the GitHub REST `releases/latest` JSON.
    pub fn from_github(v: &serde_json::Value) -> anyhow::Result<Self> {
        let tag = v["tag_name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("release json: no tag_name"))?
            .to_string();
        let version = Version::parse(&tag)
            .ok_or_else(|| anyhow::anyhow!("release tag {tag:?} is not a version"))?
            .to_string();
        let date = v["published_at"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(10)
            .collect();
        let url = v["html_url"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("{NOTES_URL}/tag/{tag}"));
        let assets = v["assets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| {
                        Some(Asset {
                            name: x["name"].as_str()?.to_string(),
                            url: x["browser_download_url"].as_str()?.to_string(),
                            size: x["size"].as_u64().unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            version,
            tag,
            date,
            url,
            assets,
        })
    }

    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }

    /// The archive for this machine, when the release ships one.
    pub fn own_archive(&self) -> Option<&Asset> {
        self.asset(&own_asset_name(&self.version)?)
    }

    /// The `.deb` / `.rpm` of the release.
    pub fn package(&self, kind: PackageKind) -> Option<&Asset> {
        let ext = match kind {
            PackageKind::Deb => ".deb",
            PackageKind::Rpm => ".rpm",
        };
        self.assets.iter().find(|a| a.name.ends_with(ext))
    }
}

// ---------------------------------------------------------------- installs

/// Linux package formats the release ships.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageKind {
    Deb,
    Rpm,
}

/// Which package the distro in `/etc/os-release` uses.
pub fn package_kind_of(os_release: &str) -> Option<PackageKind> {
    let ids: String = os_release
        .lines()
        .filter(|l| l.starts_with("ID=") || l.starts_with("ID_LIKE="))
        .map(|l| l.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    if ["debian", "ubuntu", "mint"].iter().any(|k| ids.contains(k)) {
        Some(PackageKind::Deb)
    } else if ["fedora", "rhel", "centos", "suse", "opensuse", "mageia"]
        .iter()
        .any(|k| ids.contains(k))
    {
        Some(PackageKind::Rpm)
    } else {
        None
    }
}

/// The running machine's package kind (Linux only).
pub fn package_kind() -> Option<PackageKind> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| package_kind_of(&t))
}

/// How rugen was installed, from the binary's location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    /// A self-contained folder (zip / tarball): the binary next to `motif/`.
    Portable(PathBuf),
    /// A system package or a read-only bundle: `/usr/bin/rugen` with the
    /// assets in `../share/rugen` (also Flatpak / Snap / `/opt` prefixes).
    Package(PathBuf),
    /// Neither: a development build or an unpacked tree without assets.
    Dev(PathBuf),
}

impl Install {
    /// Classify `exe` with `is_dir` answering whether a path is a directory
    /// (the real file system in [`Install::detect`], a fake one in tests).
    pub fn classify(exe: &Path, is_dir: &dyn Fn(&Path) -> bool) -> Self {
        let Some(dir) = exe.parent() else {
            return Self::Dev(exe.to_path_buf());
        };
        let s = exe.to_string_lossy().replace('\\', "/");
        let managed = [
            "/usr/",
            "/opt/",
            "/app/",
            "/snap/",
            "/nix/store/",
            "/Applications/",
        ]
        .iter()
        .any(|p| s.starts_with(p));
        if is_dir(&dir.join("motif")) && !managed {
            return Self::Portable(dir.to_path_buf());
        }
        let share = dir.parent().map(|p| p.join("share/rugen/motif"));
        if managed || share.as_deref().is_some_and(is_dir) {
            return Self::Package(exe.to_path_buf());
        }
        Self::Dev(dir.to_path_buf())
    }

    /// The running binary's install.
    pub fn detect() -> Self {
        match std::env::current_exe() {
            Ok(exe) => Self::classify(&exe, &|p: &Path| p.is_dir()),
            Err(_) => Self::Dev(PathBuf::from(".")),
        }
    }

    pub fn dir(&self) -> &Path {
        match self {
            Self::Portable(d) | Self::Dev(d) => d,
            Self::Package(exe) => exe.parent().unwrap_or(Path::new("/")),
        }
    }
}

/// Whether we can create files in `dir`.
pub fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".rugen-write-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Leftovers of the last update: Windows cannot delete the running
/// `rugen.exe`, so the previous one waits as `rugen.exe.old` until the next
/// start; staging folders of an interrupted update go too. Returns what
/// was removed.
pub fn cleanup_old(install_dir: &Path) -> Vec<PathBuf> {
    let mut gone = Vec::new();
    for name in ["rugen.exe.old", "rugen.old"] {
        let p = install_dir.join(name);
        if p.is_file() && std::fs::remove_file(&p).is_ok() {
            gone.push(p);
        }
    }
    for name in [apply::STAGE_DIR, apply::OLD_DIR] {
        let p = install_dir.join(name);
        if p.is_dir() && std::fs::remove_dir_all(&p).is_ok() {
            gone.push(p);
        }
    }
    gone
}

// ---------------------------------------------------------------- check cache

/// `cache/update.json`: when we last asked GitHub and what it said.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckCache {
    /// Unix seconds of the last successful check.
    pub checked_at: u64,
    pub latest: Option<Release>,
}

impl CheckCache {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, t);
        }
    }

    /// A new check is due at `now` (unix seconds).
    pub fn due(&self, now: u64) -> bool {
        now < self.checked_at || now - self.checked_at >= CHECK_EVERY_SECS
    }
}

/// Unix seconds now.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare() {
        assert!(is_newer("v0.8.0", "0.7.2"));
        assert!(is_newer("0.7.10", "0.7.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.7.2", "0.7.2"));
        assert!(!is_newer("0.7.1", "0.7.2"));
        assert!(is_newer("0.8.0", "0.8.0-rc1"));
        assert!(!is_newer("0.8.0-rc1", "0.8.0"));
        assert!(!is_newer("garbage", "0.7.2"));
        assert_eq!(
            Version::parse("v1.2").map(|v| v.to_string()),
            Some("1.2.0".into())
        );
        assert!(Version::parse("1.2.3.4").is_none());
    }

    #[test]
    fn asset_names_follow_the_release_workflow() {
        assert_eq!(
            asset_name("0.8.0", "windows", "x86_64").as_deref(),
            Some("rugen-0.8.0-windows-x64.zip")
        );
        assert_eq!(
            asset_name("v0.8.0", "windows", "x86").as_deref(),
            Some("rugen-0.8.0-windows-x86.zip")
        );
        assert_eq!(
            asset_name("0.8.0", "windows", "aarch64").as_deref(),
            Some("rugen-0.8.0-windows-arm64.zip")
        );
        assert_eq!(
            asset_name("0.8.0", "linux", "x86_64").as_deref(),
            Some("rugen-0.8.0-linux-x86_64.tar.gz")
        );
        assert_eq!(asset_name("0.8.0", "linux", "aarch64"), None);
        assert_eq!(
            asset_name("0.8.0", "macos", "aarch64").as_deref(),
            Some("rugen-0.8.0-macos-arm64.tar.gz")
        );
        assert_eq!(
            asset_name("0.8.0", "macos", "x86_64").as_deref(),
            Some("rugen-0.8.0-macos-x64.tar.gz")
        );
        assert_eq!(asset_name("0.8.0", "freebsd", "x86_64"), None);
        assert_eq!(ArchiveKind::of("a.zip"), Some(ArchiveKind::Zip));
        assert_eq!(ArchiveKind::of("a.tar.gz"), Some(ArchiveKind::TarGz));
        assert_eq!(ArchiveKind::of("a.deb"), None);
    }

    #[test]
    fn sha256sums_parse() {
        let text = "\
# comment
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  rugen-0.8.0-linux-x86_64.tar.gz
ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789 *rugen-0.8.0-windows-x64.zip
nothex  bad.zip
";
        let sums = parse_sha256sums(text);
        assert_eq!(sums.len(), 2);
        assert_eq!(
            expected_sha(text, "rugen-0.8.0-windows-x64.zip").as_deref(),
            Some("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789")
        );
        assert_eq!(expected_sha(text, "bad.zip"), None);
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn release_from_github_json() {
        let v = serde_json::json!({
            "tag_name": "v0.8.0",
            "published_at": "2026-10-20T08:14:42Z",
            "html_url": "https://github.com/rugen-dev/rugen-engine/releases/tag/v0.8.0",
            "assets": [
                {"name": "rugen-0.8.0-linux-x86_64.tar.gz", "browser_download_url": "https://x/a.tar.gz", "size": 12},
                {"name": "rugen_0.8.0-1_amd64.deb", "browser_download_url": "https://x/a.deb", "size": 9},
                {"name": "rugen-0.8.0-1.x86_64.rpm", "browser_download_url": "https://x/a.rpm", "size": 10},
                {"name": "SHA256SUMS", "browser_download_url": "https://x/SHA256SUMS", "size": 1}
            ]
        });
        let r = Release::from_github(&v).unwrap();
        assert_eq!(r.version, "0.8.0");
        assert_eq!(r.date, "2026-10-20");
        assert_eq!(r.assets.len(), 4);
        assert_eq!(
            r.package(PackageKind::Deb).unwrap().name,
            "rugen_0.8.0-1_amd64.deb"
        );
        assert_eq!(r.package(PackageKind::Rpm).unwrap().url, "https://x/a.rpm");
        assert!(r.asset(SUMS_ASSET).is_some());
        assert!(Release::from_github(&serde_json::json!({"tag_name": "nightly"})).is_err());
    }

    #[test]
    fn install_layouts() {
        let dirs: Vec<PathBuf> = vec![
            "/home/p/rugen/motif".into(),
            "/usr/share/rugen/motif".into(),
            "C:/Games/rugen/motif".into(),
        ];
        let is_dir = |p: &Path| {
            dirs.iter()
                .any(|d| d == &PathBuf::from(p.to_string_lossy().replace('\\', "/")))
        };
        assert_eq!(
            Install::classify(Path::new("/home/p/rugen/rugen"), &is_dir),
            Install::Portable("/home/p/rugen".into())
        );
        assert_eq!(
            Install::classify(Path::new("C:/Games/rugen/rugen.exe"), &is_dir),
            Install::Portable("C:/Games/rugen".into())
        );
        assert_eq!(
            Install::classify(Path::new("/usr/bin/rugen"), &is_dir),
            Install::Package("/usr/bin/rugen".into())
        );
        assert_eq!(
            Install::classify(Path::new("/app/bin/rugen"), &is_dir),
            Install::Package("/app/bin/rugen".into())
        );
        assert_eq!(
            Install::classify(Path::new("/home/p/src/target/debug/rugen"), &is_dir),
            Install::Dev("/home/p/src/target/debug".into())
        );
    }

    #[test]
    fn package_kinds() {
        assert_eq!(
            package_kind_of("ID=fedora\nVERSION_ID=44\n"),
            Some(PackageKind::Rpm)
        );
        assert_eq!(
            package_kind_of("ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n"),
            Some(PackageKind::Deb)
        );
        assert_eq!(
            package_kind_of("ID=opensuse-tumbleweed\n"),
            Some(PackageKind::Rpm)
        );
        assert_eq!(package_kind_of("ID=arch\n"), None);
    }

    #[test]
    fn check_cache_is_due_every_six_hours() {
        let c = CheckCache {
            checked_at: 1000,
            latest: None,
        };
        assert!(!c.due(1000 + CHECK_EVERY_SECS - 1));
        assert!(c.due(1000 + CHECK_EVERY_SECS));
        assert!(c.due(10));
        assert!(CheckCache::default().due(now_secs()));
    }

    #[test]
    fn cleanup_removes_old_binaries_and_staging() {
        let d = std::env::temp_dir().join(format!("rugen-cleanup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(apply::STAGE_DIR)).unwrap();
        std::fs::write(d.join("rugen.exe.old"), b"x").unwrap();
        std::fs::write(d.join("rugen.toml"), b"keep").unwrap();
        let gone = cleanup_old(&d);
        assert_eq!(gone.len(), 2);
        assert!(!d.join("rugen.exe.old").exists());
        assert!(!d.join(apply::STAGE_DIR).exists());
        assert!(d.join("rugen.toml").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
