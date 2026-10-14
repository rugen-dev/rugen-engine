//! What produced a replay: the engine build and the recording machine
//! (debug reports). Facts only, gathered once: OS name and version, CPU
//! architecture, model and cores, total RAM, the GPU the renderer draws
//! with, the window size and video profile.
//!
//! Privacy rule: never a hostname, user name, address, MAC, serial number
//! or home path. [`Machine::scrub`] removes the user's name, home folder
//! and the hostname should any fact happen to contain them.

use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// The build that wrote a replay.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Engine {
    /// `0.7.0`
    pub version: String,
    /// Git short hash baked in at build time (`unknown` without git).
    pub commit: String,
    /// Target triple (`x86_64-unknown-linux-gnu`).
    pub target: String,
    /// `release` or `debug`.
    pub build: String,
}

impl Engine {
    pub fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").into(),
            commit: env!("RUGEN_COMMIT").into(),
            target: env!("RUGEN_TARGET").into(),
            build: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
            .into(),
        }
    }

    /// `rugen 0.7.0 (1d5573e)`; a debug build says so.
    pub fn line(&self) -> String {
        let mut extra: Vec<&str> = Vec::new();
        if !self.commit.is_empty() && self.commit != "unknown" {
            extra.push(&self.commit);
        }
        if self.build == "debug" {
            extra.push("debug");
        }
        if extra.is_empty() {
            format!("rugen {}", self.version)
        } else {
            format!("rugen {} ({})", self.version, extra.join(", "))
        }
    }
}

/// The machine a replay was recorded on.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Machine {
    /// OS name: `Fedora Linux 44 (Workstation Edition)`, `Windows 11`, `macOS`
    pub os: String,
    /// Kernel / OS build: `7.2.9-201.fc44.x86_64`, `10.0.22631`, `15.1`
    pub os_version: String,
    /// `x86_64`, `aarch64`
    pub arch: String,
    /// CPU model (`AMD Ryzen 7 7800X3D 8-Core Processor`).
    pub cpu: String,
    /// Logical cores.
    pub cores: u32,
    /// Total RAM, GB rounded.
    pub ram_gb: u32,
    /// GPU adapter name, `headless` without a renderer.
    pub gpu: String,
    /// wgpu backend (`Vulkan`, `Metal`, `Dx12`, `Gl`), `headless` without one.
    pub backend: String,
    /// Driver and its version, when the backend says.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub driver: String,
    /// Window size in pixels (absent: headless).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<[u32; 2]>,
    /// Video profile: `desktop` or `deck`.
    pub profile: String,
}

impl Machine {
    /// This machine now (the static facts are read once).
    pub fn current() -> Self {
        let s = statics();
        let gpu = GPU.lock().ok().and_then(|g| g.clone());
        let display = DISPLAY.lock().ok().and_then(|d| *d);
        let mut m = Self {
            os: s.os.clone(),
            os_version: s.os_version.clone(),
            arch: std::env::consts::ARCH.into(),
            cpu: s.cpu.clone(),
            cores: s.cores,
            ram_gb: s.ram_gb,
            gpu: gpu.as_ref().map_or("headless".into(), |g| g.name.clone()),
            backend: gpu
                .as_ref()
                .map_or("headless".into(), |g| g.backend.clone()),
            driver: gpu.map(|g| g.driver).unwrap_or_default(),
            display,
            profile: if crate::deck::ui() { "deck" } else { "desktop" }.into(),
        };
        m.scrub(&Personal::current());
        m
    }

    /// `Fedora Linux 44 · x86_64 · AMD Ryzen 7 (16 cores) · 32 GB ·
    /// AMD Radeon RX 7800 XT (Vulkan) · 1280x720 desktop`
    pub fn line(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut push = |s: String| {
            if !s.trim().is_empty() {
                parts.push(s);
            }
        };
        push(os_short(&self.os, &self.os_version));
        push(self.arch.clone());
        if !self.cpu.is_empty() {
            push(format!("{} ({} cores)", self.cpu, self.cores));
        }
        if self.ram_gb > 0 {
            push(format!("{} GB", self.ram_gb));
        }
        if self.backend == "headless" || self.backend.is_empty() {
            push(self.gpu.clone());
        } else {
            push(format!("{} ({})", self.gpu, self.backend));
        }
        push(match self.display {
            Some([w, h]) => format!("{w}x{h} {}", self.profile),
            None => self.profile.clone(),
        });
        parts.join(" · ")
    }

    /// The few words peers and shared replays get.
    pub fn summary(&self, engine: &Engine) -> rugen_proto::MachineSummary {
        rugen_proto::MachineSummary {
            engine: format!("rugen {}", engine.version),
            commit: engine.commit.clone(),
            os: os_short(&self.os, &self.os_version),
            arch: self.arch.clone(),
            backend: self.backend.clone(),
        }
        .clean()
    }

    /// Remove the user's name, home folder and the hostname from every
    /// fact (none should hold them; this makes sure).
    pub fn scrub(&mut self, p: &Personal) {
        for f in [
            &mut self.os,
            &mut self.os_version,
            &mut self.arch,
            &mut self.cpu,
            &mut self.gpu,
            &mut self.backend,
            &mut self.driver,
            &mut self.profile,
        ] {
            *f = p.scrub(f);
        }
    }
}

/// This machine in the few words peers and servers get.
pub fn summary() -> rugen_proto::MachineSummary {
    Machine::current().summary(&Engine::current())
}

/// What must never appear in a replay: read only to be removed.
#[derive(Clone, Debug, Default)]
pub struct Personal {
    pub home: Vec<String>,
    pub user: Vec<String>,
    pub host: Vec<String>,
}

impl Personal {
    pub fn current() -> Self {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let mut host: Vec<String> = ["HOSTNAME", "COMPUTERNAME"]
            .iter()
            .filter_map(|k| env(k))
            .collect();
        if let Ok(h) = std::fs::read_to_string("/proc/sys/kernel/hostname")
            .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        {
            host.push(h.trim().to_string());
        }
        // a distribution's default hostname (`fedora`, `steamdeck`) is no
        // one's name, and removing it would eat the OS name
        let os = statics().os.to_lowercase();
        host.retain(|h| !generic_host(h, &os));
        Self {
            home: ["HOME", "USERPROFILE"]
                .iter()
                .filter_map(|k| env(k))
                .collect(),
            user: ["USER", "USERNAME", "LOGNAME"]
                .iter()
                .filter_map(|k| env(k))
                .collect(),
            host,
        }
    }

    /// `s` without any of the personal words (home first: it holds the
    /// user name). Names of 2 characters or fewer are left alone: they
    /// would eat ordinary words.
    pub fn scrub(&self, s: &str) -> String {
        let mut out = s.to_string();
        for h in &self.home {
            if h.len() > 1 {
                out = out.replace(h.as_str(), "~");
            }
        }
        // longest first: a hostname often holds the user's name
        let mut words: Vec<&str> = self
            .user
            .iter()
            .chain(&self.host)
            .map(|w| w.trim())
            .filter(|w| w.len() > 2)
            .collect();
        words.sort_by_key(|w| std::cmp::Reverse(w.len()));
        for w in words {
            out = replace_ci(&out, w, "");
        }
        out.trim().to_string()
    }

    /// The first personal word `text` holds, if any (tests, checks).
    pub fn leaks(&self, text: &str) -> Option<String> {
        let lower = text.to_lowercase();
        self.home
            .iter()
            .filter(|h| h.len() > 1)
            .chain(
                self.user
                    .iter()
                    .chain(&self.host)
                    .filter(|w| w.trim().len() > 2),
            )
            .find(|w| lower.contains(&w.trim().to_lowercase()))
            .cloned()
    }
}

/// Hostnames that systems ship with (not personal).
fn generic_host(h: &str, os: &str) -> bool {
    const DEFAULTS: [&str; 16] = [
        "localhost",
        "localhost.localdomain",
        "fedora",
        "ubuntu",
        "debian",
        "archlinux",
        "steamdeck",
        "raspberrypi",
        "manjaro",
        "pop-os",
        "nixos",
        "bazzite",
        "chimeraos",
        "linux",
        "opensuse",
        "mint",
    ];
    let h = h.trim().to_lowercase();
    DEFAULTS.contains(&h.as_str()) || (!h.is_empty() && os.contains(&h))
}

fn replace_ci(s: &str, word: &str, with: &str) -> String {
    let lower = s.to_lowercase();
    let w = word.to_lowercase();
    // byte offsets only line up when lower-casing keeps the length
    if w.is_empty() || lower.len() != s.len() || w.len() != word.len() {
        return s.replace(word, with);
    }
    let mut out = String::new();
    let mut i = 0;
    while let Some(at) = lower[i..].find(&w) {
        out.push_str(&s[i..i + at]);
        out.push_str(with);
        i += at + w.len();
    }
    out.push_str(&s[i..]);
    out
}

/// The OS in short: `Fedora Linux 44 (Workstation Edition)` gives
/// `Fedora Linux 44`; Windows and macOS get their version.
fn os_short(os: &str, version: &str) -> String {
    let base = os.split(" (").next().unwrap_or(os).trim();
    if (base.starts_with("Windows") || base == "macOS") && !version.is_empty() {
        format!("{base} {version}")
    } else {
        base.to_string()
    }
}

/// The GPU the renderer draws with (set when the window opens).
static GPU: Mutex<Option<rugen_render::AdapterSummary>> = Mutex::new(None);
/// Window size in pixels (set when the window opens or is resized).
static DISPLAY: Mutex<Option<[u32; 2]>> = Mutex::new(None);

pub fn set_gpu(a: &rugen_render::AdapterSummary) {
    if let Ok(mut g) = GPU.lock() {
        *g = Some(a.clone());
    }
}

pub fn set_display(w: u32, h: u32) {
    if let Ok(mut d) = DISPLAY.lock() {
        *d = (w > 0 && h > 0).then_some([w, h]);
    }
}

struct Statics {
    os: String,
    os_version: String,
    cpu: String,
    cores: u32,
    ram_gb: u32,
}

fn statics() -> &'static Statics {
    static S: OnceLock<Statics> = OnceLock::new();
    S.get_or_init(|| {
        let (os, os_version) = sys::os();
        Statics {
            os,
            os_version,
            cpu: cpu_brand()
                .or_else(sys::cpu)
                .map(|c| c.split_whitespace().collect::<Vec<_>>().join(" "))
                .unwrap_or_default(),
            cores: std::thread::available_parallelism().map_or(0, |n| n.get() as u32),
            ram_gb: sys::ram_bytes().map_or(0, |b| ((b as f64) / 1e9).round() as u32),
        }
    })
}

/// The CPU's own brand string (x86 CPUID leaves 0x80000002-4).
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[allow(unsafe_code, unused_unsafe)]
fn cpu_brand() -> Option<String> {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::__cpuid;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::__cpuid;
    // SAFETY: CPUID exists on every CPU rugen runs on (x86-64, i686)
    let max = unsafe { __cpuid(0x8000_0000) }.eax;
    if max < 0x8000_0004 {
        return None;
    }
    let mut bytes = Vec::with_capacity(48);
    for leaf in 0x8000_0002u32..=0x8000_0004 {
        // SAFETY: as above; the leaf is supported (checked)
        let r = unsafe { __cpuid(leaf) };
        for v in [r.eax, r.ebx, r.ecx, r.edx] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
    }
    let s = String::from_utf8_lossy(&bytes)
        .trim_matches(char::from(0))
        .trim()
        .to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn cpu_brand() -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
mod sys {
    pub fn os() -> (String, String) {
        let name = std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|t| {
                let get = |k: &str| {
                    t.lines()
                        .find_map(|l| l.strip_prefix(k))
                        .map(|v| v.trim().trim_matches('"').to_string())
                };
                get("PRETTY_NAME=").or_else(|| get("NAME="))
            })
            .unwrap_or_else(|| "Linux".into());
        let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        (name, kernel)
    }

    pub fn cpu() -> Option<String> {
        let t = std::fs::read_to_string("/proc/cpuinfo").ok()?;
        ["model name", "Model", "Hardware", "cpu model"]
            .iter()
            .find_map(|k| {
                t.lines().find_map(|l| {
                    let (a, b) = l.split_once(':')?;
                    (a.trim() == *k && !b.trim().is_empty()).then(|| b.trim().to_string())
                })
            })
    }

    pub fn ram_bytes() -> Option<u64> {
        let t = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb = t
            .lines()
            .find_map(|l| l.strip_prefix("MemTotal:"))?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()?;
        Some(kb * 1024)
    }
}

#[cfg(target_os = "macos")]
mod sys {
    fn sysctl(name: &str) -> Option<String> {
        let o = std::process::Command::new("sysctl")
            .args(["-n", name])
            .output()
            .ok()?;
        let s = String::from_utf8(o.stdout).ok()?.trim().to_string();
        (o.status.success() && !s.is_empty()).then_some(s)
    }

    pub fn os() -> (String, String) {
        (
            "macOS".into(),
            sysctl("kern.osproductversion").unwrap_or_default(),
        )
    }

    pub fn cpu() -> Option<String> {
        sysctl("machdep.cpu.brand_string")
    }

    pub fn ram_bytes() -> Option<u64> {
        sysctl("hw.memsize")?.parse().ok()
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod sys {
    #[repr(C)]
    struct OsVersionInfo {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        csd: [u16; 128],
    }

    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfo) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
    }

    pub fn os() -> (String, String) {
        let mut v = OsVersionInfo {
            size: std::mem::size_of::<OsVersionInfo>() as u32,
            major: 0,
            minor: 0,
            build: 0,
            platform: 0,
            csd: [0; 128],
        };
        // SAFETY: a correctly sized, writable OSVERSIONINFOW
        if unsafe { RtlGetVersion(&mut v) } != 0 {
            return ("Windows".into(), String::new());
        }
        // Windows 11 still reports 10.0, told apart by its build
        let name = match (v.major, v.build) {
            (10, b) if b >= 22000 => "Windows 11",
            (10, _) => "Windows 10",
            _ => "Windows",
        };
        (name.into(), format!("{}.{}.{}", v.major, v.minor, v.build))
    }

    pub fn cpu() -> Option<String> {
        std::env::var("PROCESSOR_IDENTIFIER").ok()
    }

    pub fn ram_bytes() -> Option<u64> {
        let mut m = MemoryStatusEx {
            length: std::mem::size_of::<MemoryStatusEx>() as u32,
            memory_load: 0,
            total_phys: 0,
            avail_phys: 0,
            total_page_file: 0,
            avail_page_file: 0,
            total_virtual: 0,
            avail_virtual: 0,
            avail_extended_virtual: 0,
        };
        // SAFETY: a correctly sized, writable MEMORYSTATUSEX with its length set
        let ok = unsafe { GlobalMemoryStatusEx(&mut m) } != 0;
        ok.then_some(m.total_phys)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod sys {
    pub fn os() -> (String, String) {
        (std::env::consts::OS.into(), String::new())
    }
    pub fn cpu() -> Option<String> {
        None
    }
    pub fn ram_bytes() -> Option<u64> {
        None
    }
}

/// Now in UTC, RFC 3339 (`2026-10-14T09:30:00Z`).
pub fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    rfc3339(secs)
}

/// Unix seconds as RFC 3339 UTC.
pub fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // civil from days (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// RFC 3339 UTC (as [`rfc3339`] writes it) back to Unix seconds.
pub fn parse_rfc3339(s: &str) -> Option<u64> {
    let s = s.trim().strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time
        .split(':')
        .map(|p| p.split('.').next()?.parse::<i64>().ok());
    let (hh, mm, ss) = (t.next()??, t.next()??, t.next()??);
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_timestamps() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_791_970_200), "2026-10-14T09:30:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        for s in [0, 951_782_400, 1_791_970_200, 4_102_444_799] {
            assert_eq!(parse_rfc3339(&rfc3339(s)), Some(s));
        }
        assert_eq!(parse_rfc3339("yesterday"), None);
    }

    #[test]
    fn the_engine_knows_its_build() {
        let e = Engine::current();
        assert_eq!(e.version, env!("CARGO_PKG_VERSION"));
        assert!(!e.commit.is_empty());
        assert!(e.target.contains(std::env::consts::ARCH), "{}", e.target);
        assert!(e.build == "debug" || e.build == "release");
    }

    #[test]
    fn personal_words_are_scrubbed() {
        let p = Personal {
            home: vec!["/home/alice".into()],
            user: vec!["alice".into()],
            host: vec!["alices-laptop".into()],
        };
        assert_eq!(p.scrub("/home/alice/RUGEN on ALICES-LAPTOP"), "~/RUGEN on");
        assert_eq!(p.leaks("built by Alice"), Some("alice".into()));
        assert_eq!(p.leaks("AMD Ryzen 7"), None);
        assert!(generic_host("Fedora", "fedora linux 44"));
        assert!(generic_host("steamdeck", "steamos"));
        assert!(!generic_host("alices-laptop", "fedora linux 44"));
        let mut m = Machine {
            cpu: "alice's cpu".into(),
            ..Default::default()
        };
        m.scrub(&p);
        assert_eq!(m.cpu, "'s cpu");
    }

    #[test]
    fn this_machine_in_words() {
        let m = Machine::current();
        assert_eq!(m.arch, std::env::consts::ARCH);
        assert!(!m.os.is_empty());
        assert_eq!(m.gpu, "headless", "no renderer in tests");
        let s = m.summary(&Engine::current());
        assert!(s.engine.starts_with("rugen "));
        assert_eq!(s.backend, "headless");
        let line = m.line();
        assert!(line.contains(std::env::consts::ARCH), "{line}");
    }

    /// What leaves the machine (ready reports, LAN / relay hellos, shared
    /// replays) is the summary: no CPU, GPU, RAM or driver; a shared
    /// replay drops the backend too.
    #[test]
    fn summaries_strip_the_hardware() {
        let m = Machine {
            os: "Fedora Linux 44 (Workstation Edition)".into(),
            os_version: "7.2.9-201.fc44.x86_64".into(),
            arch: "x86_64".into(),
            cpu: "AMD Ryzen 5 5500U".into(),
            cores: 12,
            ram_gb: 16,
            gpu: "AMD Radeon Graphics (RADV RENOIR)".into(),
            backend: "Vulkan".into(),
            driver: "radv Mesa 25.1.4".into(),
            display: Some([1280, 720]),
            profile: "desktop".into(),
        };
        let s = m.summary(&Engine::current());
        let text = s.encode();
        for detail in ["Ryzen", "Radeon", "RADV", "Mesa", "16", "1280", "7.2.9"] {
            assert!(!text.contains(detail), "{detail} in {text}");
        }
        assert_eq!(s.os, "Fedora Linux 44");
        let shared = serde_json::to_string(&s.shared()).unwrap();
        assert!(!shared.contains("Vulkan"), "{shared}");
        assert!(shared.contains("x86_64") && shared.contains("Fedora Linux 44"));
    }
}
