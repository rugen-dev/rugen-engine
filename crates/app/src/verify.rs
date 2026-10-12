//! Character verification: load, inspect and run a headless soak so broken or
//! abusive characters are flagged before they reach a match. The verdict is
//! stored in the cache manifest and shown on the select screen.

use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use rugen_content::{Entry, Library, Manifest, VerifyInfo};
use rugen_sim::world::Stage;
use rugen_sim::{Buttons, CharData, Rng, World};

/// Frames per soak match.
pub const SOAK_FRAMES: u32 = 3000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Verified,
    Warn,
    Broken,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Verified => "verified",
            Verdict::Warn => "warn",
            Verdict::Broken => "broken",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub load_ms: u128,
    pub states: usize,
    pub controllers: usize,
    pub unsupported: Vec<(String, usize)>,
    pub load_warnings: usize,
    pub sprite_bytes: usize,
    pub missing_sprites: usize,
    pub max_helpers: usize,
    pub max_explods: usize,
    pub max_projectiles: usize,
    /// Largest single-tick life loss as a percentage of max life.
    pub max_hit_percent: i32,
    pub sim_warnings: usize,
    pub frames: u32,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub name: String,
    pub verdict: Verdict,
    /// Human-readable problems, worst first.
    pub issues: Vec<String>,
    pub stats: Stats,
}

impl Report {
    pub fn info(&self) -> VerifyInfo {
        VerifyInfo {
            verdict: self.verdict.as_str().to_string(),
            issues: self.issues.clone(),
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
            when: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }

    pub fn print(&self) {
        let s = &self.stats;
        println!("{}: {}", self.name, self.verdict.as_str().to_uppercase());
        println!(
            "  load {} ms, {} states, {} controllers, {} sprite MB",
            s.load_ms,
            s.states,
            s.controllers,
            s.sprite_bytes / (1024 * 1024)
        );
        println!(
            "  soak {} frames: max helpers {}, explods {}, projectiles {}, biggest hit {}% of life, {} sim warnings",
            s.frames,
            s.max_helpers,
            s.max_explods,
            s.max_projectiles,
            s.max_hit_percent,
            s.sim_warnings
        );
        for (k, n) in &s.unsupported {
            println!("  unsupported {k} x{n}");
        }
        for i in &self.issues {
            println!("  ! {i}");
        }
    }
}

fn soak(
    data: Arc<CharData>,
    other: Arc<CharData>,
    fx: Option<Arc<rugen_sim::FxData>>,
    seed: u64,
    frames: u32,
    stats: &mut Stats,
    issues: &mut Vec<String>,
) -> bool {
    let mut w = World::new(Stage::default(), seed);
    if let Some(fx) = fx {
        w.round = rugen_sim::RoundState::new(fx.round.clone());
        w.fx = Some(fx);
    }
    w.add_player(data, 0, 1);
    w.add_player(other, 1, 2);
    w.players[1].ai_level = 8;
    w.start_round();
    let mut rng = Rng::new(seed ^ 0xabcdef);
    let mut last_life = [w.players[0].life, w.players[1].life];
    let mut same_state_ticks = 0u32;
    let mut last_state = (w.players[0].state_no, w.players[1].state_no);
    let mut warned_stuck = false;
    for _ in 0..frames {
        let a = Buttons::from_bits((rng.next_u64() & 0x7ff) as u16);
        let b = Buttons::default();
        let ok = catch_unwind(AssertUnwindSafe(|| w.step(&[a, b]))).is_ok();
        if !ok {
            issues.push(format!(
                "simulation panicked at frame {} (state {} / {})",
                w.frame, w.players[0].state_no, w.players[1].state_no
            ));
            return false;
        }
        stats.frames += 1;
        let helpers = w
            .players
            .iter()
            .filter(|p| !p.removed && p.helper_index > 0)
            .count();
        stats.max_helpers = stats.max_helpers.max(helpers);
        stats.max_explods = stats
            .max_explods
            .max(w.explods.iter().filter(|e| !e.removed).count());
        stats.max_projectiles = stats
            .max_projectiles
            .max(w.projs.iter().filter(|p| !p.removed).count());
        for (k, p) in w.players.iter().take(2).enumerate() {
            let drop = last_life[k] - p.life;
            if drop > 0 && p.life_max > 0 {
                let pct = drop * 100 / p.life_max;
                stats.max_hit_percent = stats.max_hit_percent.max(pct);
            }
            last_life[k] = p.life;
        }
        let st = (w.players[0].state_no, w.players[1].state_no);
        if st == last_state && w.round.round_state() == 2 {
            same_state_ticks += 1;
        } else {
            same_state_ticks = 0;
            last_state = st;
        }
        if same_state_ticks > 1800 && !warned_stuck && !matches!(st.0, 0 | 11 | 20 | 5110 | 5150) {
            warned_stuck = true;
            issues.push(format!(
                "stuck in state {} for {} frames",
                st.0, same_state_ticks
            ));
        }
        if w.round.match_over {
            // keep the soak going with a fresh round
            w.round.match_over = false;
            w.start_round();
        }
    }
    stats.sim_warnings += w.warnings.len();
    let mut kinds: BTreeSet<String> = BTreeSet::new();
    for m in &w.warnings {
        if m.contains("stuck in a loop") {
            issues.push(m.clone());
        } else if m.contains("limit") || m.contains("non-finite") || m.contains("invalid action") {
            // drop the "P<n> " prefix so helpers and roots collapse into one issue
            let stripped = m
                .strip_prefix('P')
                .and_then(|r| r.split_once(' '))
                .filter(|(n, _)| n.chars().all(|c| c.is_ascii_digit()))
                .map_or(m.as_str(), |(_, rest)| rest);
            kinds.insert(stripped.to_string());
        }
    }
    for k in kinds.into_iter().take(8) {
        issues.push(k);
    }
    true
}

/// Load, inspect and soak a character. `sparring` is a known-good opponent
/// (KFM) when available; the mirror match always runs.
pub fn verify(def: &Path, data_dir: &Path, sparring: Option<&Path>) -> Report {
    let mut issues = Vec::new();
    let mut stats = Stats::default();
    let name = def
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let t = Instant::now();
    // the verdict is about the original files, never the optimised cache
    let loaded = catch_unwind(AssertUnwindSafe(|| CharData::load_original(def, data_dir)));
    stats.load_ms = t.elapsed().as_millis();
    let data = match loaded {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => {
            issues.push(format!("does not load: {e}"));
            return Report {
                name,
                verdict: Verdict::Broken,
                issues,
                stats,
            };
        }
        Err(_) => {
            issues.push("loading panicked".into());
            return Report {
                name,
                verdict: Verdict::Broken,
                issues,
                stats,
            };
        }
    };
    let name = data.display_name.clone();
    stats.states = data.states.len();
    stats.load_warnings = data.warnings.len();
    let mut unsupported: std::collections::BTreeMap<String, usize> = Default::default();
    for st in data.states.values() {
        for c in &st.ctrls {
            stats.controllers += 1;
            if let rugen_sim::sctrl::Sctrl::Unsupported(k) = &c.op {
                *unsupported.entry(k.clone()).or_default() += 1;
            }
        }
    }
    stats.unsupported = unsupported.into_iter().collect();
    stats.sprite_bytes = data
        .sff
        .sprites
        .values()
        .map(|s| match &s.pixels {
            rugen_formats::sff::Pixels::Indexed(p) => p.len(),
            rugen_formats::sff::Pixels::Rgba(p) => p.len(),
            rugen_formats::sff::Pixels::Packed(p) => p.raw_len(),
        })
        .sum();
    // AIR frames pointing at sprites the SFF does not have
    let mut missing = BTreeSet::new();
    for a in data.air.actions.values() {
        for f in &a.frames {
            if f.group < 0 || f.number < 0 {
                continue;
            }
            let key = (f.group as u16, f.number as u16);
            if data.sff.get(key.0, key.1).is_none() {
                missing.insert(key);
            }
        }
    }
    stats.missing_sprites = missing.len();
    if !missing.is_empty() {
        let shown: Vec<String> = missing
            .iter()
            .take(5)
            .map(|(g, n)| format!("{g},{n}"))
            .collect();
        issues.push(format!(
            "{} animation frames use sprites missing from the SFF ({}{})",
            missing.len(),
            shown.join(" "),
            if missing.len() > 5 { " ..." } else { "" }
        ));
    }
    for (k, n) in &stats.unsupported {
        issues.push(format!("unsupported controller {k} ({n}x)"));
    }
    if stats.load_warnings > 0 {
        issues.push(format!(
            "{} load warnings (see rugen check)",
            stats.load_warnings
        ));
    }

    // fightfx (sparks, dust) as in a real match, so F-anim lookups are meaningful
    let fx = rugen_sim::FxData::load(data_dir).ok();
    let mut ran = soak(
        data.clone(),
        data.clone(),
        fx.clone(),
        11,
        SOAK_FRAMES,
        &mut stats,
        &mut issues,
    );
    if ran && let Some(sp) = sparring {
        if let Ok(kfm) = CharData::load_original(sp, data_dir) {
            ran = soak(
                data.clone(),
                kfm,
                fx.clone(),
                23,
                SOAK_FRAMES,
                &mut stats,
                &mut issues,
            );
        }
    }
    // the two soaks often report the same things
    let mut seen = BTreeSet::new();
    issues.retain(|i| seen.insert(i.clone()));
    if stats.max_hit_percent >= 60 {
        issues.push(format!(
            "a single hit took {}% of a life bar",
            stats.max_hit_percent
        ));
    }
    let broken = !ran || issues.iter().any(|i| i.starts_with("stuck in state"));
    let verdict = if broken {
        Verdict::Broken
    } else if issues.is_empty() {
        Verdict::Verified
    } else {
        Verdict::Warn
    };
    Report {
        name,
        verdict,
        issues,
        stats,
    }
}

/// The bundled KFM, used as a sparring partner when present.
pub fn sparring_partner(lib: &Library) -> Option<PathBuf> {
    let p = lib.root.join("reference/mugen-1.1b1/chars/kfm/kfm.def");
    if p.exists() {
        return Some(p);
    }
    let e = lib.find_char("kfm")?;
    lib.prepare_char(&e).ok().map(|(d, m)| d.join(m.def))
}

/// Prepare a character and make sure its manifest carries a verdict from
/// this engine version, verifying it when needed (first launch per version).
pub fn prepare_verified(
    lib: &Library,
    entry: &Entry,
) -> rugen_content::Result<(PathBuf, Manifest)> {
    let (dir, mut m) = lib.prepare_char(entry)?;
    let current = env!("CARGO_PKG_VERSION");
    if m.verify
        .as_ref()
        .is_some_and(|v| v.engine_version == current)
    {
        return Ok((dir, m));
    }
    log::info!("verifying {} ...", entry.id);
    let sparring = sparring_partner(lib);
    let def = dir.join(&m.def);
    let report = verify(&def, &lib.data_dir(), sparring.as_deref());
    log::info!(
        "{}: {} ({} issues)",
        report.name,
        report.verdict.as_str(),
        report.issues.len()
    );
    m.verify = Some(report.info());
    Library::save_manifest(&dir, &m)?;
    Ok((dir, m))
}
