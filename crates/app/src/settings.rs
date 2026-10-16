//! Data behind the OPTIONS (`ui/options.toml`) and CONTROLS
//! (`ui/controls.toml`) screens: setting rows grouped in tabs, their widgets
//! and help lines, and the per-action key bindings with conflicts, presets and
//! the live test pad. The TOML only lays these out.

use std::collections::HashSet;

use rugen_sim::scale::FighterSize;
use serde_json::{Value, json};
use winit::keyboard::{Key, NamedKey};

use crate::config::{Config, PlayerKeys};
use crate::input::{Device, PadKind};
use crate::lang::{t, tf};

/// `(id, tab label key, subtitle key)`; `controls` opens its own page.
pub const TABS: [(&str, &str, &str); 7] = [
    (
        "gameplay",
        "options.tab_gameplay",
        "options.tab_gameplay_sub",
    ),
    ("video", "options.tab_video", "options.tab_video_sub"),
    ("audio", "options.tab_audio", "options.tab_audio_sub"),
    ("controls", "common.controls", "options.tab_controls_sub"),
    ("online", "online.online", "options.tab_online_sub"),
    (
        "interface",
        "options.tab_interface",
        "options.tab_interface_sub",
    ),
    ("update", "options.tab_update", "options.tab_update_sub"),
];

pub const CONTROLS_TAB: usize = 3;
pub const INTERFACE_TAB: usize = 5;
pub const UPDATE_TAB: usize = 6;

const ROUND_TIMES: [i32; 4] = [30, 60, 99, 0];
const RESOLUTIONS: [(u32, u32); 4] = [(1280, 720), (1280, 800), (1600, 900), (1920, 1080)];
/// RENDER SCALE row labels, in [`crate::config::RENDER_SCALES`] order
/// (the first is drawn as `common.auto`).
const RENDER_SCALE_LABELS: [&str; 4] = ["AUTO", "100%", "75%", "50%"];

enum Widget {
    /// `max` squares, `on` filled.
    Pips(u32, u32),
    /// Options and the selected index.
    Choice(Vec<String>, Option<usize>),
    /// Fill fraction.
    Slider(f32),
    /// Fill fraction, read-only (a download).
    Progress(f32),
    /// Read-only value.
    Text,
    /// A value picked with left / right out of a list too long to show
    /// (the language): only the current one is drawn.
    Cycle(String),
    /// Opens another page.
    Link,
}

struct Row {
    id: &'static str,
    label: String,
    widget: Widget,
    /// Value text shown next to the widget (slider number, read-only value).
    text: String,
    help: String,
}

fn on_off(b: bool) -> Widget {
    Widget::Choice(
        vec![t("common.off").into(), t("common.on").into()],
        Some(usize::from(b)),
    )
}

fn percent(v: f32) -> String {
    format!("{}%", libm::roundf(v * 100.0))
}

fn rows(tab: usize, c: &Config) -> Vec<Row> {
    let row = |id, label: &str, widget, text: String, help: &str| Row {
        id,
        label: label.to_string(),
        widget,
        text,
        help: help.to_string(),
    };
    match TABS[tab].0 {
        "gameplay" => vec![
            row(
                "ai",
                t("options.computer_level"),
                Widget::Pips(8, c.game.ai_level.clamp(0, 8) as u32),
                String::new(),
                t("options.computer_level_help"),
            ),
            row(
                "round_time",
                t("options.round_time"),
                Widget::Choice(
                    vec!["30".into(), "60".into(), "99".into(), "@inf".into()],
                    ROUND_TIMES
                        .iter()
                        .position(|t| *t == c.game.round_time.max(0)),
                ),
                String::new(),
                t("options.round_time_help"),
            ),
            row(
                "life",
                t("online.life"),
                Widget::Slider(
                    (crate::config::clamp_life(c.game.life) - crate::config::LIFE_MIN) as f32
                        / (crate::config::LIFE_MAX - crate::config::LIFE_MIN) as f32,
                ),
                crate::config::clamp_life(c.game.life).to_string(),
                t("options.life_help"),
            ),
            row(
                "air_jumps",
                t("options.air_jumps"),
                Widget::Choice(
                    (0..4).map(|i| i.to_string()).collect(),
                    usize::try_from(c.game.air_jumps).ok().filter(|i| *i < 4),
                ),
                String::new(),
                t("options.air_jumps_help"),
            ),
            row(
                "air_dash",
                t("options.air_dash"),
                on_off(c.game.air_dash),
                String::new(),
                t("options.air_dash_help"),
            ),
            row(
                "fast_fall",
                t("options.fast_fall"),
                on_off(c.game.fast_fall),
                String::new(),
                t("options.fast_fall_help"),
            ),
            row(
                "arena_ai",
                t("options.arena_ai"),
                on_off(c.game.arena_ai),
                String::new(),
                t("options.arena_ai_help"),
            ),
            row(
                "fighter_size",
                t("online.fighter_size"),
                Widget::Choice(
                    FighterSize::ALL.iter().map(|v| v.label().into()).collect(),
                    FighterSize::ALL
                        .iter()
                        .position(|v| *v == c.game.fighter_size),
                ),
                String::new(),
                t("options.fighter_size_help"),
            ),
            row(
                "zoom_out",
                t("options.camera_zoom_out"),
                Widget::Slider((c.camera.zoom_out - 0.25) / 0.75),
                format!("{:.2}", c.camera.zoom_out),
                t("options.camera_zoom_out_help"),
            ),
            row(
                "zoom_in",
                t("options.camera_zoom_in"),
                Widget::Slider((c.camera.zoom_in - 0.6) / 1.0),
                format!("{:.2}", c.camera.zoom_in),
                t("options.camera_zoom_in_help"),
            ),
        ],
        "video" => vec![
            row(
                "fullscreen",
                t("title.fullscreen"),
                on_off(c.video.fullscreen),
                String::new(),
                t("options.fullscreen_help"),
            ),
            row(
                "resolution",
                t("options.window_size"),
                Widget::Choice(
                    RESOLUTIONS
                        .iter()
                        .map(|(w, h)| format!("{w}x{h}"))
                        .collect(),
                    RESOLUTIONS
                        .iter()
                        .position(|r| *r == (c.video.width, c.video.height)),
                ),
                String::new(),
                t("options.window_size_help"),
            ),
            row(
                "profile",
                t("common.device"),
                Widget::Choice(
                    crate::deck::PROFILES
                        .iter()
                        .map(|p| crate::deck::label(p).to_string())
                        .collect(),
                    crate::deck::PROFILES
                        .iter()
                        .position(|p| p.eq_ignore_ascii_case(c.video.profile.trim())),
                ),
                String::new(),
                t("options.device_help"),
            ),
            row(
                "render_scale",
                t("options.render_scale"),
                Widget::Choice(
                    RENDER_SCALE_LABELS
                        .iter()
                        .enumerate()
                        .map(|(i, s)| if i == 0 { t("common.auto") } else { s }.to_string())
                        .collect(),
                    Some(c.video.render_scale_index()),
                ),
                String::new(),
                t("options.render_scale_help"),
            ),
            row(
                "mouse",
                t("options.mouse_in_menus"),
                on_off(c.input.mouse),
                String::new(),
                t("options.mouse_in_menus_help"),
            ),
        ],
        "audio" => vec![
            row(
                "master",
                t("options.master_volume"),
                Widget::Slider(c.audio.master),
                percent(c.audio.master),
                t("options.master_volume_help"),
            ),
            row(
                "music",
                t("options.music_volume"),
                Widget::Slider(c.audio.music),
                percent(c.audio.music),
                t("options.music_volume_help"),
            ),
        ],
        "controls" => vec![
            row(
                "keys1",
                t("options.bindings_1p"),
                Widget::Link,
                String::new(),
                t("options.bindings_1p_help"),
            ),
            row(
                "keys2",
                t("options.bindings_2p"),
                Widget::Link,
                String::new(),
                t("options.bindings_2p_help"),
            ),
        ],
        "online" => {
            let show = |s: &str| {
                if s.trim().is_empty() {
                    t("options.not_set").to_string()
                } else {
                    s.trim().to_uppercase()
                }
            };
            let mut rows = vec![
                row(
                    "delay",
                    t("options.input_delay"),
                    {
                        let (opts, sel) = c.net.input_delay.choice();
                        Widget::Choice(opts, Some(sel))
                    },
                    c.net.input_delay.to_string(),
                    t("options.input_delay_help"),
                ),
                row(
                    "direct",
                    t("options.direct_connection"),
                    on_off(c.net.direct),
                    String::new(),
                    t("options.direct_connection_help"),
                ),
                row(
                    "server",
                    t("options.server"),
                    Widget::Text,
                    show(&c.net.server),
                    t("options.server_help"),
                ),
                row(
                    "good_conn",
                    t("options.good_connections_only"),
                    on_off(c.net.good_connections_only),
                    String::new(),
                    t("options.good_connections_only_help"),
                ),
            ];
            if crate::link::supported() {
                rows.push(row(
                    "links",
                    t("options.register_rugen_links"),
                    Widget::Link,
                    crate::link::status(),
                    t("options.register_rugen_links_help"),
                ));
            }
            rows
        }
        "update" => update_rows(c),
        "interface" => {
            // AUTO (the OS language, named) then every shipped language in
            // its own name
            let auto = format!(
                "{} ({})",
                t("common.auto"),
                crate::lang::label(crate::lang::detect())
            );
            let i = crate::lang::picker_index(&c.interface.lang);
            let current = match i.checked_sub(1).and_then(|i| crate::lang::LANGS.get(i)) {
                Some((_, n)) => (*n).to_string(),
                None => auto,
            };
            vec![row(
                "language",
                t("options.language"),
                Widget::Cycle(current.clone()),
                current,
                t("options.language_help"),
            )]
        }
        _ => Vec::new(),
    }
}

/// The UPDATE tab: the running and latest versions, CHECK NOW, UPDATE NOW
/// (its progress while it runs, RESTART once installed) or the package
/// link, the launch check toggle and the release notes.
fn update_rows(c: &Config) -> Vec<Row> {
    use rugen_update::{Install, Phase};
    let row = |id, label: &str, widget, text: String, help: &str| Row {
        id,
        label: label.to_string(),
        widget,
        text,
        help: help.to_string(),
    };
    let snap = crate::update::snapshot();
    let up = crate::update::get();
    let opened = crate::update::opened();
    let copied = |url: &str| {
        if opened.as_deref() == Some(url) {
            t("options.copied")
        } else {
            ""
        }
    };
    let mut out = vec![row(
        "update_version",
        t("options.version"),
        Widget::Text,
        env!("CARGO_PKG_VERSION").to_string(),
        t("options.version_help"),
    )];
    let Some(snap) = snap else {
        out.push(row(
            "update_latest",
            t("options.latest"),
            Widget::Text,
            t("common.off").into(),
            t("options.updater_off_help"),
        ));
        return out;
    };
    let newer = snap.newer().cloned();
    let (latest, latest_help) = match (&snap.phase, &snap.latest) {
        (Phase::Checking, _) => (
            t("options.checking").to_string(),
            t("options.asking_github_help").to_string(),
        ),
        (Phase::Failed, None) => (
            t("options.check_failed").to_string(),
            tf(
                "options.could_not_reach_github",
                &[("error", &snap.error.clone().unwrap_or_default())],
            ),
        ),
        (_, Some(r)) if newer.is_some() => (
            format!("{} - {}", r.version, r.date),
            tf("options.new_release_help", &[("version", &r.version)]),
        ),
        (_, Some(r)) => (
            tf("options.version_up_to_date", &[("version", &r.version)]),
            tf("options.up_to_date_help", &[("date", &r.date)]),
        ),
        _ => (
            t("options.not_checked_yet").to_string(),
            t("options.no_check_yet_help").to_string(),
        ),
    };
    out.push(row(
        "update_latest",
        t("options.latest"),
        Widget::Text,
        latest,
        &latest_help,
    ));
    let checking = snap.phase == Phase::Checking;
    out.push(row(
        "update_check",
        t("options.check_now"),
        if checking { Widget::Text } else { Widget::Link },
        if checking { "..." } else { "" }.into(),
        t("options.check_now_help"),
    ));
    if let Some(r) = &newer {
        let install = up.as_ref().map(|u| u.install().clone());
        let blocker: Option<String> = up.as_ref().and_then(|u| u.blocker());
        match (install, blocker.as_deref()) {
            (Some(Install::Package(_)), _) => {
                let kind = rugen_update::package_kind();
                let (label, what) = match kind {
                    Some(rugen_update::PackageKind::Deb) => (t("options.get_the_deb"), ".deb"),
                    Some(rugen_update::PackageKind::Rpm) => (t("options.get_the_rpm"), ".rpm"),
                    None => (t("options.get_the_package"), t("options.package")),
                };
                let url = kind
                    .and_then(|k| r.package(k))
                    .map(|a| a.url.clone())
                    .unwrap_or_else(|| r.url.clone());
                out.push(row(
                    "update_package",
                    label,
                    Widget::Link,
                    copied(&url).into(),
                    &tf(
                        "options.package_update_help",
                        &[("what", &what), ("version", &r.version)],
                    ),
                ));
            }
            (_, Some(why)) if !matches!(snap.phase, Phase::Ready) => {
                out.push(row(
                    "update_now",
                    t("options.update_now"),
                    Widget::Text,
                    t("options.unavailable").into(),
                    &tf(
                        "options.update_unavailable_help",
                        &[("why", &capitalize(why)), ("version", &r.version)],
                    ),
                ));
            }
            _ => {
                let (label, widget, text, help): (&str, Widget, String, String) = match snap.phase {
                    Phase::Downloading => (
                        t("options.update_now"),
                        Widget::Progress(snap.progress),
                        match snap.total {
                            Some(t) if t > 0 => format!("{}%", (snap.progress * 100.0) as u32),
                            _ => format!("{} MB", snap.received >> 20),
                        },
                        tf("options.downloading_help", &[("version", &r.version)]),
                    ),
                    Phase::Verifying => (
                        t("options.update_now"),
                        Widget::Progress(1.0),
                        t("options.verifying").into(),
                        t("options.verifying_help").into(),
                    ),
                    Phase::Installing => (
                        t("options.update_now"),
                        Widget::Progress(1.0),
                        t("options.installing").into(),
                        t("options.installing_help").into(),
                    ),
                    Phase::Ready => (
                        t("options.restart"),
                        Widget::Link,
                        tf("options.version_installed", &[("version", &r.version)]),
                        t("options.restart_help").into(),
                    ),
                    Phase::Failed => (
                        t("options.update_now"),
                        Widget::Link,
                        t("options.failed_retry").into(),
                        tf(
                            "options.update_failed_help",
                            &[("error", &snap.error.clone().unwrap_or_default())],
                        ),
                    ),
                    _ => (
                        t("options.update_now"),
                        Widget::Link,
                        String::new(),
                        tf(
                            "options.update_now_help",
                            &[
                                ("version", &r.version),
                                (
                                    "size",
                                    &r.own_archive()
                                        .map(|a| format!("{} MB", (a.size + (1 << 19)) >> 20))
                                        .unwrap_or_default(),
                                ),
                            ],
                        ),
                    ),
                };
                out.push(row("update_now", label, widget, text, &help));
            }
        }
    }
    out.push(row(
        "update_auto",
        t("options.check_at_launch"),
        on_off(c.update.check),
        String::new(),
        t("options.check_at_launch_help"),
    ));
    let notes = snap
        .latest
        .as_ref()
        .map(|r| r.url.clone())
        .unwrap_or_else(|| rugen_update::NOTES_URL.to_string());
    out.push(row(
        "update_notes",
        t("options.release_notes"),
        Widget::Link,
        if copied(&notes).is_empty() {
            "RUGEN.DEV".into()
        } else {
            t("options.copied").into()
        },
        t("options.release_notes_help"),
    ));
    out
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn row_json(r: &Row) -> Value {
    let adjustable = matches!(
        r.widget,
        Widget::Pips(..) | Widget::Choice(..) | Widget::Slider(_) | Widget::Cycle(_)
    );
    let (kind, max, on, options, sel, frac) = match &r.widget {
        Widget::Pips(m, o) => ("pips", *m, *o, Vec::new(), -1, 0.0),
        Widget::Choice(opts, s) => (
            "choice",
            0,
            0,
            opts.clone(),
            s.map_or(-1, |i| i as i64),
            0.0,
        ),
        Widget::Slider(f) | Widget::Progress(f) => {
            ("slider", 0, 0, Vec::new(), -1, f.clamp(0.0, 1.0))
        }
        Widget::Text | Widget::Cycle(_) => ("text", 0, 0, Vec::new(), -1, 0.0),
        Widget::Link => ("link", 0, 0, Vec::new(), -1, 0.0),
    };
    // plain value for screens that only show `{item.value}`
    let value = match &r.widget {
        Widget::Pips(_, o) if r.text.is_empty() => o.to_string(),
        Widget::Choice(opts, Some(i)) => opts[*i].replace("@inf", t("options.inf")),
        Widget::Link => ">".into(),
        Widget::Cycle(v) => v.clone(),
        _ => r.text.clone(),
    };
    let id = r.id;
    json!({
        "id": id,
        "label": r.label,
        "value": value,
        "text": r.text,
        "has_text": !r.text.is_empty(),
        "help": r.help,
        "widget": kind,
        "is_pips": kind == "pips",
        "is_choice": kind == "choice",
        "is_slider": kind == "slider",
        "is_text": kind == "text",
        "is_link": kind == "link",
        "adjustable": adjustable,
        "max": max,
        "on": on,
        "options": options,
        "sel": sel,
        "frac": frac,
        "press": if adjustable { format!("cycle:{id}") } else { format!("open:{id}") },
        "left": if adjustable { format!("adjust:{id}:-1") } else { String::new() },
        "right": if adjustable { format!("adjust:{id}:1") } else { String::new() },
    })
}

/// Display name of a theme folder.
pub fn theme_label(id: &str) -> String {
    match id {
        "ink" => "SHONEN INK".into(),
        "dojo" => "DOJO".into(),
        other => other.to_uppercase(),
    }
}

/// The INTERFACE theme cards: bundled themes first (in house order), then
/// the player's own, then the card that opens the themes folder.
fn theme_cards(themes: &[crate::themes::Card], c: &Config) -> Vec<Value> {
    let rank = |t: &crate::themes::Card| match t.id.as_str() {
        _ if t.user => 2,
        "ink" => 0,
        "dojo" => 1,
        _ => 1,
    };
    let mut sorted: Vec<&crate::themes::Card> = themes.iter().collect();
    sorted.sort_by_key(|t| rank(t));
    let mut out: Vec<Value> = sorted
        .into_iter()
        .map(|card| {
            let active = card.id.eq_ignore_ascii_case(c.video.theme.trim());
            let preview = card
                .preview
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            json!({
                "id": card.id,
                "label": if card.user { tf("options.theme_yours", &[("name", &card.label)]) } else { card.label.clone() },
                "name": card.label,
                "sub": if card.user { t("options.live_reload_on") } else { t("common.bundled") },
                "user": card.user,
                "is_add": false,
                "has_preview": !preview.is_empty(),
                // a frame of its own (not active, not the folder card) /
                // its name in the frame when it ships no preview
                "plain": !active,
                "show_name": preview.is_empty(),
                "preview": preview,
                "active": active,
                "press": format!("theme:{}", card.id),
            })
        })
        .collect();
    out.push(json!({
        "id": "",
        "label": t("options.open_themes_folder"),
        "name": "",
        "sub": t("options.add_your_own"),
        "user": false,
        "is_add": true,
        "has_preview": false,
        "plain": false,
        "show_name": false,
        "preview": "",
        "active": false,
        "press": "themes:open",
    }));
    out
}

/// Everything `options.toml` binds to.
pub fn options_data(
    tab: usize,
    c: &Config,
    themes: &[crate::themes::Card],
    focused: Option<&str>,
) -> Value {
    let tab = tab.min(TABS.len() - 1);
    let tabs: Vec<Value> = TABS
        .iter()
        .enumerate()
        .map(|(i, (id, label, _))| {
            json!({
                "id": id,
                "label": t(label),
                "active": i == tab,
                "ext": i == CONTROLS_TAB,
                "press": format!("tab:{i}"),
            })
        })
        .collect();
    let rows = rows(tab, c);
    let theme_cards: Vec<Value> = if tab == INTERFACE_TAB {
        theme_cards(themes, c)
    } else {
        Vec::new()
    };
    let index = |prefix: &str| {
        focused
            .and_then(|f| f.strip_prefix(prefix))
            .and_then(|r| r.split(']').next())
            .and_then(|i| i.parse::<usize>().ok())
    };
    let help = if let Some(i) = index("rows[") {
        rows.get(i).map_or("", |r| r.help.as_str()).to_string()
    } else if let Some(i) = index("themes[") {
        match theme_cards.get(i) {
            Some(card) if card["is_add"] == json!(true) => {
                t("options.themes_folder_help").to_string()
            }
            Some(card) if card["user"] == json!(true) => tf(
                "options.your_theme_help",
                &[("theme", &card["id"].as_str().unwrap_or_default())],
            ),
            _ => t("options.use_this_theme_help").to_string(),
        }
    } else if tab == CONTROLS_TAB {
        t("options.controls_page_help").to_string()
    } else {
        // what the tab holds (its subtitle already says what it is for)
        let inside = match TABS[tab].0 {
            "gameplay" => t("options.tab_gameplay_help"),
            "video" => t("options.tab_video_help"),
            "audio" => t("options.tab_audio_help"),
            "online" => t("options.tab_online_help"),
            "interface" => t("options.tab_interface_help"),
            "update" => t("options.tab_update_help"),
            _ => "",
        };
        tf("options.pick_a_category_help", &[("inside", &inside)])
            .trim_start()
            .to_string()
    };
    // directions read as arrows outside the controls page
    let (left, left_glyph) = (String::new(), "left");
    let (right, right_glyph) = (String::new(), "right");
    json!({
        "tabs": tabs,
        "tab": { "id": TABS[tab].0, "title": t(TABS[tab].1), "subtitle": t(TABS[tab].2), "index": tab },
        "rows": rows.iter().map(row_json).collect::<Vec<_>>(),
        "show_rows": !rows.is_empty(),
        // INTERFACE: the LANGUAGE row sits above the theme cards
        "rows_h": if tab == INTERFACE_TAB { 48 } else { 414 },
        "themes_y": if tab == INTERFACE_TAB { 118 } else { 62 },
        "themes_grid_y": if tab == INTERFACE_TAB { 144 } else { 88 },
        "themes_y_dojo": if tab == INTERFACE_TAB { 130 } else { 74 },
        "themes_grid_y_dojo": if tab == INTERFACE_TAB { 156 } else { 100 },
        "themes": theme_cards,
        "show_themes": tab == INTERFACE_TAB,
        "help": help,
        "keys": { "left": left, "left_glyph": left_glyph, "right": right, "right_glyph": right_glyph },
        "capture": { "active": false, "prompt": "", "progress": "" },
    })
}

fn step(v: f32, d: f32, lo: f32, hi: f32) -> f32 {
    // snap to the step grid so repeated presses never drift
    (libm::roundf((v + d) / d.abs().max(1e-6)) * d.abs()).clamp(lo, hi)
}

/// Move setting `id` by `dir`; `wrap` cycles choices past their ends (enter key).
/// Returns false for unknown / read-only rows.
pub fn adjust(c: &mut Config, id: &str, dir: i32, wrap: bool) -> bool {
    let d = dir as f32;
    let cycle = |cur: usize, n: usize| -> usize {
        let next = cur as i32 + dir;
        if wrap {
            next.rem_euclid(n as i32) as usize
        } else {
            next.clamp(0, n as i32 - 1) as usize
        }
    };
    match id {
        "master" => c.audio.master = step(c.audio.master, 0.1 * d, 0.0, 1.0),
        "music" => c.audio.music = step(c.audio.music, 0.1 * d, 0.0, 1.0),
        "zoom_out" => c.camera.zoom_out = step(c.camera.zoom_out, 0.05 * d, 0.25, 1.0),
        "zoom_in" => c.camera.zoom_in = step(c.camera.zoom_in, 0.05 * d, 0.6, 1.6),
        "life" => {
            let (lo, hi) = (crate::config::LIFE_MIN, crate::config::LIFE_MAX);
            let next = crate::config::clamp_life(c.game.life) + dir * crate::config::LIFE_STEP;
            c.game.life = if wrap && next > hi {
                lo
            } else {
                next.clamp(lo, hi)
            };
        }
        "fullscreen" => c.video.fullscreen = !c.video.fullscreen,
        "air_dash" => c.game.air_dash = !c.game.air_dash,
        "fast_fall" => c.game.fast_fall = !c.game.fast_fall,
        "arena_ai" => c.game.arena_ai = !c.game.arena_ai,
        "mouse" => c.input.mouse = !c.input.mouse,
        "ai" => {
            c.game.ai_level = if wrap && c.game.ai_level + dir > 8 {
                1
            } else {
                (c.game.ai_level + dir).clamp(1, 8)
            }
        }
        "delay" => c.net.input_delay = c.net.input_delay.step(dir, wrap),
        "good_conn" => c.net.good_connections_only = !c.net.good_connections_only,
        "direct" => c.net.direct = !c.net.direct,
        "update_auto" => c.update.check = !c.update.check,
        "fighter_size" => {
            let cur = FighterSize::ALL
                .iter()
                .position(|v| *v == c.game.fighter_size)
                .unwrap_or(3);
            c.game.fighter_size = FighterSize::ALL[cycle(cur, FighterSize::ALL.len())];
        }
        "air_jumps" => c.game.air_jumps = cycle(c.game.air_jumps.clamp(0, 3) as usize, 4) as i32,
        "round_time" => {
            let cur = ROUND_TIMES
                .iter()
                .position(|t| *t == c.game.round_time.max(0))
                .unwrap_or(2);
            c.game.round_time = ROUND_TIMES[cycle(cur, ROUND_TIMES.len())];
        }
        "profile" => {
            let cur = crate::deck::PROFILES
                .iter()
                .position(|p| p.eq_ignore_ascii_case(c.video.profile.trim()))
                .unwrap_or(0);
            c.video.profile = crate::deck::PROFILES[cycle(cur, 3)].into();
        }
        "render_scale" => {
            let n = crate::config::RENDER_SCALES.len();
            c.video.render_scale =
                crate::config::RENDER_SCALES[cycle(c.video.render_scale_index(), n)].into();
        }
        "language" => {
            let cur = crate::lang::picker_index(&c.interface.lang);
            c.interface.lang =
                crate::lang::picker_value(cycle(cur, crate::lang::LANGS.len() + 1)).into();
        }
        "resolution" => {
            let cur = RESOLUTIONS
                .iter()
                .position(|r| *r == (c.video.width, c.video.height))
                .unwrap_or(0);
            let (w, h) = RESOLUTIONS[cycle(cur, RESOLUTIONS.len())];
            c.video.width = w;
            c.video.height = h;
        }
        _ => return false,
    }
    true
}

/// A mouse pick on row `id`'s widget (`pick:<id>:<value>`): `value` is the
/// pips filled up to the cursor (1..), the choice index (0..) or the
/// slider fraction (0..1). Sliders snap to their keyboard step. True when
/// something changed.
pub fn pick(c: &mut Config, id: &str, value: &str) -> bool {
    let Ok(v) = value.trim().parse::<f32>() else {
        return false;
    };
    if !v.is_finite() {
        return false;
    }
    let i = v.max(0.0) as usize;
    let before = c.clone();
    let snap = |lo: f32, hi: f32, step_: f32| {
        let x = lo + v.clamp(0.0, 1.0) * (hi - lo);
        (libm::roundf(x / step_) * step_).clamp(lo, hi)
    };
    let on = i >= 1;
    match id {
        "master" => c.audio.master = snap(0.0, 1.0, 0.05),
        "music" => c.audio.music = snap(0.0, 1.0, 0.05),
        "zoom_out" => c.camera.zoom_out = snap(0.25, 1.0, 0.05),
        "zoom_in" => c.camera.zoom_in = snap(0.6, 1.6, 0.05),
        "life" => {
            let (lo, hi) = (
                crate::config::LIFE_MIN as f32,
                crate::config::LIFE_MAX as f32,
            );
            c.game.life = snap(lo, hi, crate::config::LIFE_STEP as f32) as i32;
        }
        "ai" => c.game.ai_level = (i as i32).clamp(1, 8),
        "fullscreen" => c.video.fullscreen = on,
        "air_dash" => c.game.air_dash = on,
        "fast_fall" => c.game.fast_fall = on,
        "arena_ai" => c.game.arena_ai = on,
        "mouse" => c.input.mouse = on,
        "good_conn" => c.net.good_connections_only = on,
        "direct" => c.net.direct = on,
        "update_auto" => c.update.check = on,
        "delay" => {
            let max = crate::config::InputDelay::MAX;
            c.net.input_delay = crate::config::InputDelay((i > 0).then(|| (i - 1).min(max)));
        }
        "fighter_size" => {
            c.game.fighter_size = FighterSize::ALL[i.min(FighterSize::ALL.len() - 1)];
        }
        "air_jumps" => c.game.air_jumps = i.min(3) as i32,
        "round_time" => c.game.round_time = ROUND_TIMES[i.min(ROUND_TIMES.len() - 1)],
        "profile" => c.video.profile = crate::deck::PROFILES[i.min(2)].into(),
        "render_scale" => {
            let n = crate::config::RENDER_SCALES.len();
            c.video.render_scale = crate::config::RENDER_SCALES[i.min(n - 1)].into();
        }
        "language" => c.interface.lang = crate::lang::picker_value(i).into(),
        "resolution" => {
            let (w, h) = RESOLUTIONS[i.min(RESOLUTIONS.len() - 1)];
            c.video.width = w;
            c.video.height = h;
        }
        _ => return false,
    }
    *c != before
}

// ---------------------------------------------------------------- controls

/// `(id, label, group key)` in config order (Buttons order + START). The
/// label is a key for the words (`controls.up`), the letter itself for the
/// buttons: [`action_label`] resolves it.
pub const ACTIONS: [(&str, &str, &str); 13] = [
    ("up", "controls.up", "controls.move"),
    ("down", "controls.down", "controls.move"),
    ("left", "controls.left", "controls.move"),
    ("right", "controls.right", "controls.move"),
    ("a", "A", "controls.low_row"),
    ("b", "B", "controls.low_row"),
    ("c", "C", "controls.low_row"),
    ("x", "X", "controls.top_row"),
    ("y", "Y", "controls.top_row"),
    ("z", "Z", "controls.top_row"),
    ("s", "S", "controls.fighter_extra"),
    ("start", "common.start", "controls.pause_menus"),
    ("tag", "common.tag", "controls.tag_teams"),
];

/// The label of action `i` in the active language (`UP`, `A`, `START`).
pub fn action_label(i: usize) -> &'static str {
    let label = ACTIONS[i].1;
    if label.contains('.') { t(label) } else { label }
}

/// One-press layouts for a whole player.
pub fn presets() -> [(&'static str, PlayerKeys); 3] {
    let keys = |n: [&str; 13]| {
        let mut k = PlayerKeys::p1();
        for (i, name) in n.iter().enumerate() {
            k.set(i, name);
        }
        k
    };
    [
        ("AZERTY ZQSD", PlayerKeys::p1()),
        (
            "QWERTY WASD",
            keys([
                "w", "s", "a", "d", "j", "k", "l", "u", "i", "o", ";", "p", "n",
            ]),
        ),
        (t("controls.arrows_numpad"), PlayerKeys::p2()),
    ]
}

fn same_key(a: &str, b: &str) -> bool {
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

/// Per action of `player`: the conflict text ("ALSO B", "ALSO 2P A") if its key is used twice.
pub fn conflicts(keys: [&PlayerKeys; 2], player: usize) -> [Option<String>; 13] {
    let me = keys[player].names();
    let other = keys[1 - player].names();
    std::array::from_fn(|i| {
        let k = me[i];
        if let Some(j) = other.iter().position(|o| same_key(k, o)) {
            return Some(tf(
                "controls.also_np_action",
                &[("p", &(2 - player)), ("action", &action_label(j))],
            ));
        }
        (0..ACTIONS.len())
            .find(|&j| j != i && same_key(k, me[j]))
            .map(|j| tf("controls.also_action", &[("action", &action_label(j))]))
    })
}

/// The controls page: which player and device are shown, the row listening
/// for a key, keys held right now (for the test pad).
#[derive(Default)]
pub struct Controls {
    pub player: usize,
    /// 0 = keyboard, n = gamepad n-1.
    pub device: usize,
    pub listen: Option<usize>,
    held: HashSet<String>,
    /// Connected gamepads: name, family and held raw mask (bit per slot).
    pub pads: Vec<(String, PadKind, u16)>,
}

fn button(mask: u16, i: usize) -> bool {
    mask & (1 << i) != 0
}

impl Controls {
    pub fn new(player: usize) -> Self {
        Self {
            player: player.min(1),
            ..Default::default()
        }
    }

    fn keys_mut<'a>(&self, c: &'a mut Config) -> &'a mut PlayerKeys {
        if self.player == 0 {
            &mut c.input.p1
        } else {
            &mut c.input.p2
        }
    }

    fn on_pad(&self) -> Option<&(String, PadKind, u16)> {
        self.device.checked_sub(1).and_then(|i| self.pads.get(i))
    }

    /// Track held keys and handle the page keys (Tab, Backspace/Delete, F5).
    /// Returns true for Enter: the host presses the focused node.
    pub fn key(&mut self, c: &mut Config, key: &Key, pressed: bool, focused: Option<&str>) -> bool {
        if let Some(name) = crate::input::key_name(key) {
            if pressed {
                self.held.insert(name.to_lowercase());
            } else {
                self.held.remove(&name.to_lowercase());
            }
        }
        if !pressed || self.listen.is_some() {
            return false;
        }
        let row = focused
            .and_then(|f| f.strip_prefix("binds["))
            .and_then(|r| r.split(']').next())
            .and_then(|i| i.parse::<usize>().ok())
            .filter(|i| *i < ACTIONS.len());
        match key {
            Key::Named(NamedKey::Tab) => {
                self.player = 1 - self.player;
                self.listen = None;
            }
            Key::Named(NamedKey::Backspace | NamedKey::Delete) => match row {
                Some(i) if self.on_pad().is_none() => self.keys_mut(c).set(i, ""),
                _ => return false,
            },
            Key::Named(NamedKey::F5) => {
                *self.keys_mut(c) = if self.player == 0 {
                    PlayerKeys::p1()
                } else {
                    PlayerKeys::p2()
                };
            }
            Key::Named(NamedKey::Enter) => return true,
            _ => return false,
        }
        false
    }

    /// The key that ends a listen: binds it (Escape cancels). Returns true when something changed.
    pub fn bind(&mut self, c: &mut Config, key: &Key) -> bool {
        let Some(i) = self.listen else {
            return false;
        };
        if *key == Key::Named(NamedKey::Escape) {
            self.listen = None;
            return true;
        }
        let Some(name) = crate::input::key_name(key) else {
            return false;
        };
        self.keys_mut(c).set(i, &name);
        self.listen = None;
        true
    }

    /// Page actions: `player:<n>`, `device:next`, `listen:<i>`, `preset:<i>`, `defaults`.
    /// Returns true when the action was ours.
    pub fn action(&mut self, c: &mut Config, a: &str) -> bool {
        let (head, arg) = a.split_once(':').unwrap_or((a, ""));
        match head {
            "player" => {
                self.player = arg.parse::<usize>().unwrap_or(0).min(1);
                self.listen = None;
            }
            "device" => {
                self.device = (self.device + 1) % (self.pads.len() + 1);
                self.listen = None;
            }
            "listen" => {
                if self.on_pad().is_none() {
                    let i = arg.parse::<usize>().unwrap_or(0).min(ACTIONS.len() - 1);
                    self.listen = if self.listen == Some(i) {
                        None
                    } else {
                        Some(i)
                    };
                }
            }
            "preset" => {
                if let Some((_, k)) = presets().into_iter().nth(arg.parse().unwrap_or(0)) {
                    *self.keys_mut(c) = k;
                    self.listen = None;
                }
            }
            "defaults" => {
                *self.keys_mut(c) = if self.player == 0 {
                    PlayerKeys::p1()
                } else {
                    PlayerKeys::p2()
                };
            }
            _ => return false,
        }
        true
    }

    /// Everything `controls.toml` binds to.
    pub fn data(&self, c: &Config, ticks: u64) -> Value {
        let keys = [&c.input.p1, &c.input.p2];
        let me = keys[self.player];
        let names = me.names();
        let pad = self.on_pad();
        let conflicts = if pad.is_some() {
            Default::default()
        } else {
            conflicts(keys, self.player)
        };
        let lit: [bool; ACTIONS.len()] = std::array::from_fn(|i| match pad {
            Some((_, _, b)) => button(*b, i),
            None => !names[i].is_empty() && self.held.contains(&names[i].to_lowercase()),
        });
        let device = match pad {
            Some((_, kind, _)) => Device::Pad(*kind),
            None => Device::Keyboard,
        };
        let cap = |i: usize| -> (String, &'static str) {
            let c = crate::keylabel::cap(&c.input, self.player, i, device);
            (c.text, c.glyph)
        };
        let binds: Vec<Value> = ACTIONS
            .iter()
            .enumerate()
            .map(|(i, (id, _, group))| {
                let label = action_label(i);
                let listening = self.listen == Some(i);
                let (text, glyph) = cap(i);
                let conflict = conflicts[i].clone();
                let dir = i < 4;
                json!({
                    "id": id,
                    "label": label,
                    "group": if pad.is_some() { t("controls.fixed_on_pad") } else { t(group) },
                    "icon": if dir || *id == "start" {
                        String::new()
                    } else if *id == "tag" {
                        "T".to_string()
                    } else {
                        label.to_string()
                    },
                    "icon_glyph": if dir { *id } else if *id == "start" { "tri_right" } else { "" },
                    "round": !dir && *id != "start",
                    "square": dir || *id == "start",
                    "cap": text,
                    "glyph": glyph,
                    "listening": listening,
                    "lit": lit[i] && !listening,
                    "hot": lit[i] || listening,
                    "conflict": conflict.is_some() && !listening,
                    "clean": conflict.is_none() && !listening,
                    "conflict_text": conflict.clone().unwrap_or_default(),
                    "show_group": conflict.is_none() && !listening,
                    "show_conflict": conflict.is_some() && !listening,
                    "spaced": i == 4 || i == 10,
                    "is_start": *id == "start",
                    "press": format!("listen:{i}"),
                })
            })
            .collect();
        let n_conf = conflicts.iter().filter(|c| c.is_some()).count();
        let mut padj = serde_json::Map::new();
        for (i, (id, _, _)) in ACTIONS.iter().enumerate() {
            let (text, glyph) = cap(i);
            let label = action_label(i);
            padj.insert(
                id.to_string(),
                json!({ "cap": text, "glyph": glyph, "lit": lit[i], "label": label }),
            );
        }
        let dx = (i32::from(lit[3]) - i32::from(lit[2])) * 18;
        let dy = (i32::from(lit[1]) - i32::from(lit[0])) * 18;
        padj.insert("dx".into(), json!(dx));
        padj.insert("dy".into(), json!(dy));
        let presets: Vec<Value> = presets()
            .iter()
            .enumerate()
            .map(|(i, (label, k))| {
                json!({
                    "label": label,
                    "active": pad.is_none() && k.names() == me.names(),
                    "press": format!("preset:{i}"),
                })
            })
            .collect();
        let device_label = match pad {
            Some((name, _, _)) => {
                let mut n = name.to_uppercase();
                n.truncate(18);
                n
            }
            None => t("controls.keyboard").into(),
        };
        let other = 1 - self.player;
        let players: Vec<Value> = (0..2)
            .map(|p| {
                json!({
                    "label": format!("{}P", p + 1),
                    "active": p == self.player,
                    "press": format!("player:{p}"),
                })
            })
            .collect();
        json!({
            "player": self.player,
            "players": players,
            "device": {
                "label": device_label,
                "glyph": if pad.is_some() { "pad" } else { "kbd" },
                "is_pad": pad.is_some(),
                "count": self.pads.len() + 1,
                "press": "device:next",
            },
            "binds": binds,
            "conflicts": {
                "count": n_conf,
                "any": n_conf > 0,
                "none": n_conf == 0,
                "text": if n_conf == 0 { t("controls.no_conflicts").to_string() } else if n_conf == 1 { t("controls.one_conflict").to_string() } else { tf("controls.n_conflicts", &[("n", &n_conf)]) },
            },
            "pad": Value::Object(padj),
            "presets": presets,
            "listening": self.listen.is_some(),
            "fixed": pad.is_some(),
            "other": format!("{}P", other + 1),
            "blink": ticks / 20 % 2 == 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// INTERFACE: bundled themes first, the player's own after them (with
    /// a live reload tag), then the OPEN THEMES FOLDER card.
    #[test]
    fn user_themes_get_cards_after_the_bundled_ones() {
        use crate::themes::Card;
        let card = |id: &str, user: bool| Card {
            id: id.into(),
            label: id.to_uppercase(),
            user,
            preview: None,
        };
        let mut c = Config::default();
        c.video.theme = "neon".into();
        let d = options_data(
            INTERFACE_TAB,
            &c,
            &[card("neon", true), card("dojo", false), card("ink", false)],
            Some("themes[2].card"),
        );
        let ids: Vec<&str> = d["themes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["ink", "dojo", "neon", ""]);
        let neon = &d["themes"][2];
        assert_eq!(neon["label"], "NEON (YOURS)");
        assert_eq!(neon["sub"], "LIVE RELOAD ON");
        assert_eq!(neon["active"], true);
        assert_eq!(neon["show_name"], true, "no preview: its name in the frame");
        assert_eq!(d["themes"][3]["is_add"], true);
        assert_eq!(d["themes"][3]["press"], "themes:open");
        assert!(d["help"].as_str().unwrap().contains("themes/neon"));
    }

    #[test]
    fn conflicts_within_and_across_players() {
        let mut c = Config::default();
        assert!(
            conflicts([&c.input.p1, &c.input.p2], 0)
                .iter()
                .all(Option::is_none)
        );
        c.input.p1.a = "k".into(); // same as B
        c.input.p1.start = "ArrowUp".into(); // 2P up
        let cf = conflicts([&c.input.p1, &c.input.p2], 0);
        assert_eq!(cf[4].as_deref(), Some("ALSO B"));
        assert_eq!(cf[5].as_deref(), Some("ALSO A"));
        assert_eq!(cf[11].as_deref(), Some("ALSO 2P UP"));
        let cf2 = conflicts([&c.input.p1, &c.input.p2], 1);
        assert_eq!(cf2[0].as_deref(), Some("ALSO 1P START"));
    }

    #[test]
    fn rebind_one_action_and_presets() {
        let mut c = Config::default();
        let mut ctl = Controls::new(0);
        assert!(ctl.action(&mut c, "listen:4"));
        assert_eq!(ctl.listen, Some(4));
        assert!(ctl.bind(&mut c, &Key::Character("p".into())));
        assert_eq!(c.input.p1.a, "p");
        assert_eq!(c.input.p1.b, "k", "only that action changes");
        assert!(ctl.listen.is_none());
        ctl.action(&mut c, "preset:1");
        assert_eq!(c.input.p1.up, "w");
        assert_eq!(c.input.p1.start, "p");
        assert_eq!(c.input.p1.s, ";");
        ctl.key(&mut c, &Key::Named(NamedKey::F5), true, None);
        assert_eq!(c.input.p1.up, "z");
        ctl.key(
            &mut c,
            &Key::Named(NamedKey::Backspace),
            true,
            Some("binds[2].row"),
        );
        assert_eq!(c.input.p1.left, "");
        let d = ctl.data(&c, 0);
        assert_eq!(d["binds"][2]["cap"], "-");
    }

    /// A tab's help line never repeats the panel subtitle.
    #[test]
    fn tab_help_does_not_repeat_the_subtitle() {
        let c = Config::default();
        for (i, (_, _, subtitle)) in TABS.iter().enumerate() {
            let d = options_data(i, &c, &[], Some(&format!("tabs[{i}]")));
            let help = d["help"].as_str().unwrap_or_default().to_string();
            assert!(!help.is_empty(), "tab {i}");
            assert!(!help.contains(subtitle), "tab {i}: {help}");
        }
    }

    #[test]
    fn adjust_steps_and_wraps() {
        let mut c = Config::default();
        c.camera.zoom_out = 0.6;
        adjust(&mut c, "zoom_out", 1, false);
        assert!((c.camera.zoom_out - 0.65).abs() < 1e-4);
        assert_eq!(c.game.fighter_size, FighterSize::Normal, "default");
        // the mouse picks a segment, a pip or a slider position directly
        let mut m = Config::default();
        assert!(pick(&mut m, "fighter_size", "1"));
        assert_eq!(m.game.fighter_size, FighterSize::Chibi);
        assert!(!pick(&mut m, "fighter_size", "1"), "same pick, no change");
        assert!(pick(&mut m, "ai", "7"));
        assert_eq!(m.game.ai_level, 7);
        assert!(pick(&mut m, "air_dash", "1"));
        assert!(m.game.air_dash);
        assert!(pick(&mut m, "air_dash", "0"));
        assert!(!m.game.air_dash);
        assert!(pick(&mut m, "zoom_out", "0.5"));
        assert!(
            (m.camera.zoom_out - 0.65).abs() < 1e-6,
            "0.25 + 0.375 snaps to 0.65"
        );
        assert!(pick(&mut m, "master", "0.333"));
        assert!((m.audio.master - 0.35).abs() < 1e-6);
        assert!(pick(&mut m, "delay", "3"));
        assert_eq!(m.net.input_delay.0, Some(2));
        assert!(pick(&mut m, "delay", "0"));
        assert_eq!(m.net.input_delay.0, None, "AUTO");
        assert!(pick(&mut m, "render_scale", "3"));
        assert_eq!(m.video.render_scale, "0.5");
        assert!(!pick(&mut m, "render_scale", "3"), "same pick, no change");
        assert!(adjust(&mut m, "render_scale", 1, true));
        assert_eq!(m.video.render_scale, "auto", "wraps past 50%");
        assert!(!pick(&mut m, "server", "1"), "not a widget row");
        assert!(!pick(&mut m, "ai", "x"));
        adjust(&mut c, "fighter_size", 1, false);
        assert_eq!(
            c.game.fighter_size,
            FighterSize::Normal,
            "clamped at the end"
        );
        adjust(&mut c, "fighter_size", 1, true);
        assert_eq!(c.game.fighter_size, FighterSize::None, "enter wraps");
        adjust(&mut c, "fighter_size", 1, false);
        assert_eq!(c.game.fighter_size, FighterSize::Chibi);
        // LIFE: 1000..10000 in steps of 500, 5000 by default
        assert_eq!(c.game.life, 5000);
        adjust(&mut c, "life", 1, false);
        assert_eq!(c.game.life, 5500);
        c.game.life = 10000;
        adjust(&mut c, "life", 1, false);
        assert_eq!(c.game.life, 10000, "clamped");
        adjust(&mut c, "life", 1, true);
        assert_eq!(c.game.life, 1000, "enter wraps");
        assert!(pick(&mut c, "life", "0.5"));
        assert_eq!(c.game.life, 5500);
        assert_eq!(crate::config::clamp_life(7777), 8000);
        c.game.round_time = 99;
        adjust(&mut c, "round_time", 1, false);
        assert_eq!(c.game.round_time, 0, "99 -> infinite");
        adjust(&mut c, "round_time", 1, false);
        assert_eq!(c.game.round_time, 0, "clamped at the end");
        adjust(&mut c, "round_time", 1, true);
        assert_eq!(c.game.round_time, 30, "enter wraps");
        let d = options_data(0, &c, &[], Some("rows[7].row"));
        assert!(
            d["help"]
                .as_str()
                .unwrap()
                .starts_with("Normal: fighters already")
        );
        let d = options_data(0, &c, &[], Some("rows[8].row"));
        assert_eq!(d["rows"][1]["sel"], 0);
        assert!(
            d["help"]
                .as_str()
                .unwrap()
                .starts_with("How far the camera")
        );
    }
}
