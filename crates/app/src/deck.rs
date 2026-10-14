//! The Steam Deck profile (`[video] profile`): `auto` (the default) turns
//! it on when the game runs on a Steam Deck, `deck` / `desktop` force it.
//!
//! On the Deck: a first launch writes a fullscreen 1280x800 rugen.toml,
//! every hint names the Deck's buttons from the first frame (A, B, X, Y,
//! L1 / R1, L2 / R2, the menu button) instead of keyboard keys, and the
//! pads Steam Input presents (an Xbox-like virtual pad) are named as the
//! Deck's own controls. Pausing a local match when the window loses focus
//! (the Steam button, a suspend) and ignoring the clock jump after a
//! resume hold for every profile.

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether the menus draw for the Deck right now (see [`set_ui`]).
static UI: AtomicBool = AtomicBool::new(false);

/// Turn the Deck's menu text on or off (the shell calls it whenever the
/// config changes): screens then draw `fontN` at scale 1 with the theme's
/// `fontN.deck` (see [`DECK_FONT_SLOT`]).
pub fn set_ui(on: bool) {
    UI.store(on, Ordering::Relaxed);
}

/// The Deck's menu text is on.
pub fn ui() -> bool {
    UI.load(Ordering::Relaxed)
}

/// The profile whose node overrides screens apply (`deck = { ... }` in a
/// screen TOML, see [`rugen_ui::ScreenDef::parse_for`]).
pub fn screen_profile() -> Option<&'static str> {
    ui().then_some("deck")
}

/// `system.def` `fontN.deck` loads into slot `N + DECK_FONT_SLOT`: the
/// bigger small text of the Deck's 7" screen, used in place of `fontN`
/// wherever a screen draws `fontN` at scale 1 (scaled-up uses keep the
/// theme's own lettering). A theme without the key draws `fontN`.
pub const DECK_FONT_SLOT: i32 = 100;

/// The font slot a screen's `font` at `scale` draws with: the Deck variant
/// when the Deck's menu text is on, the theme has one (`has`) and the text
/// is drawn 1:1.
pub fn font_slot(font: i32, scale: f32, has: impl Fn(i32) -> bool) -> i32 {
    slot_for(ui(), font, scale, has)
}

fn slot_for(on: bool, font: i32, scale: f32, has: impl Fn(i32) -> bool) -> i32 {
    let deck = font + DECK_FONT_SLOT;
    if on && (scale - 1.0).abs() < 1e-3 && has(deck) {
        deck
    } else {
        font
    }
}

/// `[video] profile` values.
pub const PROFILES: [&str; 3] = ["auto", "deck", "desktop"];

/// True when running on a Steam Deck: Steam's `SteamDeck=1`, else the
/// board name of the LCD (`Jupiter`) or OLED (`Galileo`) model.
pub fn detected() -> bool {
    if std::env::var("SteamDeck").is_ok_and(|v| v.trim() == "1") {
        return true;
    }
    let read = |f: &str| {
        std::fs::read_to_string(format!("/sys/devices/virtual/dmi/id/{f}")).unwrap_or_default()
    };
    is_deck_board(&read("sys_vendor"), &read("board_name"))
}

fn is_deck_board(vendor: &str, board: &str) -> bool {
    vendor.trim().eq_ignore_ascii_case("valve")
        && matches!(
            board.trim().to_ascii_lowercase().as_str(),
            "jupiter" | "galileo"
        )
}

/// Whether the Deck profile is on for `profile` (`auto` asks [`detected`]).
pub fn active(profile: &str) -> bool {
    match profile.trim().to_ascii_lowercase().as_str() {
        "deck" => true,
        "desktop" => false,
        _ => detected(),
    }
}

/// OPTIONS label of a profile.
pub fn label(profile: &str) -> &'static str {
    match profile.trim().to_ascii_lowercase().as_str() {
        "deck" => "STEAM DECK",
        "desktop" => "DESKTOP",
        _ => "AUTO",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_and_boards() {
        assert!(active("deck"));
        assert!(!active("desktop"));
        assert!(is_deck_board("Valve\n", "Jupiter\n"));
        assert!(is_deck_board("Valve", "Galileo"));
        assert!(!is_deck_board("LENOVO", "Jupiter"));
        assert!(!is_deck_board("Valve", "Something"));
        assert_eq!(label("AUTO"), "AUTO");
        assert_eq!(label("deck"), "STEAM DECK");
    }

    /// Both themes give the Deck its bigger text and every screen parses
    /// with the Deck's overrides laid over it.
    #[test]
    fn themes_carry_the_deck_text() {
        let themes = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../motif/themes");
        for t in ["ink", "dojo"] {
            let dir = themes.join(t);
            let def = std::fs::read_to_string(dir.join("system.def")).unwrap();
            for slot in [1, 2] {
                let want = format!("font{slot}.deck = font/{}.def", crate::fontgen::DECK_FONT);
                assert!(def.lines().any(|l| l.trim() == want), "{t}: no {want}");
            }
            assert!(
                dir.join("font")
                    .join(format!("{}.sff", crate::fontgen::DECK_FONT))
                    .exists()
            );
            let mut n = 0;
            for e in std::fs::read_dir(dir.join("ui")).unwrap() {
                let p = e.unwrap().path();
                if p.extension().is_some_and(|x| x == "toml") {
                    let text = std::fs::read_to_string(&p).unwrap();
                    rugen_ui::ScreenDef::parse_for(&text, Some("deck"))
                        .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                    n += 1;
                }
            }
            assert!(n >= 10, "{t}: {n} screens");
        }
    }

    #[test]
    fn deck_fonts_replace_one_to_one_text_only() {
        let has = |slot| slot == 101;
        assert_eq!(slot_for(false, 1, 1.0, has), 1);
        assert_eq!(slot_for(true, 1, 1.0, has), 101);
        assert_eq!(slot_for(true, 1, 2.0, has), 1, "scaled lettering stays");
        assert_eq!(slot_for(true, 2, 1.0, has), 2, "no deck variant");
    }
}
