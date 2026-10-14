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
}
