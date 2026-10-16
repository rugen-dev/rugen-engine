//! PRACTICE (`ui/practice.toml`): the title's one entry for everything
//! offline that is not a fight to win. Three cards: TRAINING (its pause
//! menu holds the LESSONS, the dummy and the display tools), WATCH (CPU vs
//! CPU, team modes too) and the fighter DEBUGGER.

use serde_json::{Value, json};

use crate::lang::t;

/// The cards in order. `f1` is what opens the hitboxes (always F1 today).
pub fn cards() -> Value {
    let row = |name: &str, text: &str, hot: bool| json!({ "name": name, "text": text, "hot": hot, "cold": !hot });
    json!([
        {
            "id": "training",
            "title": t("practice.training"),
            "desc": t("practice.training_desc"),
            "rows": [
                row(t("practice.lessons"), t("practice.lessons_desc"), true),
                row(t("practice.dummy"), t("practice.dummy_desc"), false),
                row(t("practice.display"), t("practice.display_desc"), false),
            ],
            "chip_l": t("practice.pick_2_fighters_stage"),
            "chip_r": t("practice.f1_hitboxes"),
            "new": false,
            "action": "mode:training",
        },
        {
            "id": "watch",
            "title": t("practice.watch"),
            "desc": t("practice.watch_desc"),
            "rows": [],
            "chip_l": t("practice.pick_2_fighters_stage"),
            "chip_r": t("practice.cpu_level_options"),
            "new": false,
            "action": "mode:watch",
        },
        {
            "id": "debugger",
            "title": t("practice.debugger"),
            "desc": t("practice.debugger_desc"),
            "rows": [],
            "chip_l": t("practice.pick_1_or_2_fighters"),
            "chip_r": "SPACE · . · ,",
            "new": true,
            "action": "mode:debugger",
        },
    ])
}

/// The line under the cards.
pub fn note() -> &'static str {
    t("practice.note")
}

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
