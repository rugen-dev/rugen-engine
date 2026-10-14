//! PRACTICE (`ui/practice.toml`): the title's one entry for everything
//! offline that is not a fight to win. Three cards: TRAINING (its pause
//! menu holds the LESSONS, the dummy and the display tools), WATCH (CPU vs
//! CPU, team modes too) and the fighter DEBUGGER.

use serde_json::{Value, json};

/// The cards in order. `f1` is what opens the hitboxes (always F1 today).
pub fn cards() -> Value {
    let row = |name: &str, text: &str, hot: bool| json!({ "name": name, "text": text, "hot": hot, "cold": !hot });
    json!([
        {
            "id": "training",
            "title": "TRAINING",
            "desc": "FIGHT A DUMMY. EVERYTHING IS IN THE PAUSE MENU:",
            "rows": [
                row("LESSONS", "BASICS, THEN SPECIALS", true),
                row("DUMMY", "STANCE, BLOCK, RECORD", false),
                row("DISPLAY", "HITBOXES, FRAMES, INPUTS", false),
            ],
            "chip_l": "PICK 2 FIGHTERS · STAGE",
            "chip_r": "F1 HITBOXES",
            "new": false,
            "action": "mode:training",
        },
        {
            "id": "watch",
            "title": "WATCH",
            "desc": "TWO CPU FIGHTERS PLAY WHILE YOU WATCH. TEAM MODES WORK HERE TOO.",
            "rows": [],
            "chip_l": "PICK 2 FIGHTERS · STAGE",
            "chip_r": "CPU LEVEL: OPTIONS",
            "new": false,
            "action": "mode:watch",
        },
        {
            "id": "debugger",
            "title": "DEBUGGER",
            "desc": "STEP A FIGHTER FRAME BY FRAME: STATE, VARIABLES AND EVERY CONTROLLER THAT RAN OR FAILED. STEP BACK 10 S.",
            "rows": [],
            "chip_l": "PICK 1 OR 2 FIGHTERS",
            "chip_r": "SPACE · . · ,",
            "new": true,
            "action": "mode:debugger",
        },
    ])
}

/// The line under the cards.
pub const NOTE: &str = "OFFLINE ONLY · TEAM MODES IN WATCH";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_cards_in_the_mockup_order() {
        let c = cards();
        let ids: Vec<&str> = c
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["training", "watch", "debugger"]);
        assert_eq!(c[2]["new"], true, "the debugger is tagged NEW");
        assert_eq!(c[0]["rows"][0]["name"], "LESSONS");
    }
}
