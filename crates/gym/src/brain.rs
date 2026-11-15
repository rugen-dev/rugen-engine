//! A player's plan: up to ten steps it thought out ahead, with the branch
//! to take on each outcome, which the match then runs by itself at full
//! speed while the player thinks about the next one.
//!
//! ```json
//! {"steps": [
//!    {"card": "approach", "until": "distance<45"},
//!    {"card": "strong_kung_fu_palm", "hit": 3, "blocked": 5, "missed": 6},
//!    {"card": "light_kung_fu_knee", "hit": 4, "else": 5},
//!    {"card": "triple_kung_fu_palm", "if": "power>=1000", "else": 5, "next": 1},
//!    {"card": "block_low", "for_ms": 500, "next": 1},
//!    {"card": "back_off", "until": "distance>80", "next": 1}],
//!  "rules": [{"if": "i_am_hit", "goto": 5}, {"if": "he_jumps", "goto": 3}],
//!  "say": "pressure him, guard when it fails"}
//! ```
//!
//! A step is a card of the deck. A stance is held `until` a condition holds
//! or `for_ms`; a move is played (with `for_ms`, its last input is held
//! that long: a move that charges while held) (when `if` holds, else the `else` branch)
//! and ends as `hit`, `blocked` or `missed` (which is also a move that did
//! not come out). A branch names the step to go to (1 is the first; `"end"`
//! ends the plan); without one the next step follows. A rule jumps to a step
//! when its condition becomes true, whatever the step: `i_am_hit` once the
//! fighter is out of the hit, the others while a stance is held.
//!
//! A plan is taken whole or not at all: an unknown card, condition or step
//! number refuses it, with the reason.

use serde_json::Value;

use crate::cards::Card;

/// The most steps a plan has.
pub const MAX_STEPS: usize = 10;

/// What a plan can test. The same list a player is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cond {
    DistanceLt(i32),
    DistanceGt(i32),
    PowerGe(i32),
    PowerLt(i32),
    /// The fighter's own life, percent.
    LifeLt(i32),
    /// The other fighter's life, percent.
    HisLifeLt(i32),
    HeAttacks,
    HeJumps,
    HeIsDown,
    HeIsIdle,
    /// The other fighter is being hit (and not guarding it).
    HeIsHit,
    /// This fighter is in the air.
    IAmInAir,
    /// The other fighter's attack is past its last attack box: it cannot
    /// hit any more and is not free yet.
    HeIsRecovering,
    /// The other fighter is higher / lower than this one by a body's height.
    HeIsAbove,
    HeIsBelow,
    /// One of the other fighter's projectiles is out.
    ProjectileIncoming,
    /// The wall is right behind this fighter / the other one.
    IAmCornered,
    HeIsCornered,
    /// The other fighter's body is within the reach of this card's hitbox
    /// (a card of the deck, by its place in it; a move with no measured
    /// reach, like a projectile, is always in range).
    InRange(usize),
    /// Rules only.
    IAmHit,
}

/// What the conditions are tested on, this tick.
#[derive(Clone, Debug, Default)]
pub struct View {
    /// The gap between the two bodies (0: touching).
    pub distance: i32,
    pub power: i32,
    pub life_pct: i32,
    pub his_life_pct: i32,
    pub he_attacks: bool,
    pub he_jumps: bool,
    pub he_is_down: bool,
    pub he_is_idle: bool,
    pub he_is_hit: bool,
    pub i_am_in_air: bool,
    pub he_is_recovering: bool,
    pub he_is_above: bool,
    pub he_is_below: bool,
    pub projectile_incoming: bool,
    pub i_am_cornered: bool,
    pub he_is_cornered: bool,
    /// Per card of the deck: the other fighter is within its reach.
    pub in_range: Vec<bool>,
}

impl Cond {
    /// The conditions a plan may use, as a player writes them.
    pub const LIST: [&'static str; 21] = [
        "distance<N",
        "distance>N",
        "power>=N",
        "power<N",
        "life<N% (your own)",
        "his_life<N%",
        "he_attacks",
        "he_jumps",
        "he_is_down",
        "he_is_idle",
        "he_is_hit",
        "i_am_in_air",
        "he_is_recovering (his attack can no longer hit and he is not free yet)",
        "he_is_above",
        "he_is_below",
        "projectile_incoming",
        "i_am_cornered",
        "he_is_cornered",
        "in_range (of this step's move; on a stance: of the next move in the plan)",
        "in_range:<card id>",
        "i_am_hit (rules only)",
    ];

    pub fn parse(text: &str) -> Result<Self, String> {
        let t: String = text
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        let num = |s: &str| {
            s.trim_end_matches('%')
                .parse::<i32>()
                .map_err(|_| format!("`{text}`: a number is expected"))
        };
        Ok(match t.as_str() {
            "he_attacks" => Cond::HeAttacks,
            "he_jumps" => Cond::HeJumps,
            "he_is_down" => Cond::HeIsDown,
            "he_is_idle" => Cond::HeIsIdle,
            "he_is_hit" => Cond::HeIsHit,
            "i_am_in_air" => Cond::IAmInAir,
            "he_is_recovering" => Cond::HeIsRecovering,
            "he_is_above" => Cond::HeIsAbove,
            "he_is_below" => Cond::HeIsBelow,
            "projectile_incoming" => Cond::ProjectileIncoming,
            "i_am_cornered" => Cond::IAmCornered,
            "he_is_cornered" => Cond::HeIsCornered,
            "i_am_hit" => Cond::IAmHit,
            _ => {
                if let Some(n) = t.strip_prefix("distance<") {
                    Cond::DistanceLt(num(n.trim_start_matches('='))?)
                } else if let Some(n) = t.strip_prefix("distance>") {
                    Cond::DistanceGt(num(n.trim_start_matches('='))?)
                } else if let Some(n) = t.strip_prefix("power>=") {
                    Cond::PowerGe(num(n)?)
                } else if let Some(n) = t.strip_prefix("power<") {
                    Cond::PowerLt(num(n.trim_start_matches('='))?)
                } else if let Some(n) = t.strip_prefix("his_life<") {
                    Cond::HisLifeLt(num(n.trim_start_matches('='))?)
                } else if let Some(n) = t.strip_prefix("life<") {
                    Cond::LifeLt(num(n.trim_start_matches('='))?)
                } else {
                    return Err(format!("`{text}` is not a condition"));
                }
            }
        })
    }

    pub fn holds(self, v: &View) -> bool {
        match self {
            Cond::DistanceLt(n) => v.distance < n,
            Cond::DistanceGt(n) => v.distance > n,
            Cond::PowerGe(n) => v.power >= n,
            Cond::PowerLt(n) => v.power < n,
            Cond::HisLifeLt(n) => v.his_life_pct < n,
            Cond::LifeLt(n) => v.life_pct < n,
            Cond::HeAttacks => v.he_attacks,
            Cond::HeJumps => v.he_jumps,
            Cond::HeIsDown => v.he_is_down,
            Cond::HeIsIdle => v.he_is_idle,
            Cond::HeIsHit => v.he_is_hit,
            Cond::IAmInAir => v.i_am_in_air,
            Cond::HeIsRecovering => v.he_is_recovering,
            Cond::HeIsAbove => v.he_is_above,
            Cond::HeIsBelow => v.he_is_below,
            Cond::ProjectileIncoming => v.projectile_incoming,
            Cond::IAmCornered => v.i_am_cornered,
            Cond::HeIsCornered => v.he_is_cornered,
            Cond::InRange(card) => v.in_range.get(card).copied().unwrap_or(true),
            Cond::IAmHit => false,
        }
    }

    pub fn text(self) -> String {
        match self {
            Cond::DistanceLt(n) => format!("distance<{n}"),
            Cond::DistanceGt(n) => format!("distance>{n}"),
            Cond::PowerGe(n) => format!("power>={n}"),
            Cond::PowerLt(n) => format!("power<{n}"),
            Cond::HisLifeLt(n) => format!("his_life<{n}%"),
            Cond::LifeLt(n) => format!("life<{n}%"),
            Cond::HeAttacks => "he_attacks".into(),
            Cond::HeJumps => "he_jumps".into(),
            Cond::HeIsDown => "he_is_down".into(),
            Cond::HeIsIdle => "he_is_idle".into(),
            Cond::HeIsHit => "he_is_hit".into(),
            Cond::IAmInAir => "i_am_in_air".into(),
            Cond::HeIsRecovering => "he_is_recovering".into(),
            Cond::HeIsAbove => "he_is_above".into(),
            Cond::HeIsBelow => "he_is_below".into(),
            Cond::ProjectileIncoming => "projectile_incoming".into(),
            Cond::IAmCornered => "i_am_cornered".into(),
            Cond::HeIsCornered => "he_is_cornered".into(),
            Cond::InRange(_) => "in_range".into(),
            Cond::IAmHit => "i_am_hit".into(),
        }
    }
}

/// Where a branch leads: a step (0-based here), or the plan's end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum To {
    Step(usize),
    End,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// The card, as its place in the deck.
    pub card: usize,
    /// A stance: held until this holds...
    pub until: Option<Cond>,
    /// ...or for this many ticks.
    pub for_ticks: Option<u32>,
    /// A move: played only when this holds.
    pub gate: Option<Cond>,
    pub next: Option<To>,
    pub hit: Option<To>,
    pub blocked: Option<To>,
    pub missed: Option<To>,
    pub otherwise: Option<To>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rule {
    pub cond: Cond,
    pub goto: To,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Plan {
    pub steps: Vec<Step>,
    pub rules: Vec<Rule>,
}

impl Plan {
    /// A plan from a player's answer, for a fighter with this deck.
    pub fn parse(v: &Value, deck: &[Card]) -> Result<Self, String> {
        let steps = v["steps"]
            .as_array()
            .filter(|s| !s.is_empty())
            .ok_or("no `steps`: a plan is {\"steps\":[{\"card\":...},...]}")?;
        if steps.len() > MAX_STEPS {
            return Err(format!("{} steps: at most {MAX_STEPS}", steps.len()));
        }
        let n = steps.len();
        let to = |v: &Value, what: &str| -> Result<Option<To>, String> {
            match v {
                Value::Null => Ok(None),
                Value::String(s) if s.eq_ignore_ascii_case("end") => Ok(Some(To::End)),
                _ => match v.as_u64().or_else(|| v.as_str()?.trim().parse().ok()) {
                    Some(k) if (1..=n as u64).contains(&k) => Ok(Some(To::Step(k as usize - 1))),
                    _ => Err(format!(
                        "`{what}`: {v} is not a step (1 to {n}, or \"end\")"
                    )),
                },
            }
        };
        let is_move = |s: &Value| {
            let id = s["card"].as_str().unwrap_or("");
            deck.iter().position(|c| c.id == id && c.kind != "stance")
        };
        // `at`: the step the condition is on (a rule has none). `in_range`
        // is of that step's move, or on a stance of the next move after it
        let cond = |v: &Value, at: Option<usize>| -> Result<Option<Cond>, String> {
            let text = match v {
                Value::Null => return Ok(None),
                _ => v.as_str().ok_or(format!("{v} is not a condition"))?,
            };
            let t: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            let t = t.to_ascii_lowercase();
            let Some(rest) = t.strip_prefix("in_range") else {
                return Cond::parse(text).map(Some);
            };
            let card = match rest.strip_prefix(':') {
                Some(id) => deck
                    .iter()
                    .position(|c| c.id == id)
                    .ok_or(format!("`{text}`: no card `{id}` in the deck"))?,
                None => at
                    .and_then(|k| steps[k..].iter().find_map(is_move))
                    .ok_or(format!("`{text}`: of which move? write in_range:<card id>"))?,
            };
            Ok(Some(Cond::InRange(card)))
        };
        let mut out = Plan::default();
        for (k, s) in steps.iter().enumerate() {
            let at = |e: String| format!("step {}: {e}", k + 1);
            let id = s["card"].as_str().ok_or_else(|| at("no `card`".into()))?;
            let card = deck
                .iter()
                .position(|c| c.id == id)
                .ok_or_else(|| at(format!("no card `{id}` in the deck")))?;
            let for_ticks = match &s["for_ms"] {
                Value::Null => None,
                ms => Some(
                    (ms.as_f64()
                        .ok_or_else(|| at("`for_ms` is a number".into()))?
                        * 0.06) as u32,
                ),
            };
            let step = Step {
                card,
                until: cond(&s["until"], Some(k)).map_err(at)?,
                for_ticks,
                gate: cond(&s["if"], Some(k)).map_err(at)?,
                next: to(&s["next"], "next").map_err(at)?,
                hit: to(&s["hit"], "hit").map_err(at)?,
                blocked: to(&s["blocked"], "blocked").map_err(at)?,
                missed: to(&s["missed"], "missed").map_err(at)?,
                otherwise: to(&s["else"], "else").map_err(at)?,
            };
            if [step.until, step.gate].contains(&Some(Cond::IAmHit)) {
                return Err(at("`i_am_hit` is for rules".into()));
            }
            // an approach with nothing to end it walks until the next move
            // of the plan is in range
            let mut step = step;
            if id == "approach" && step.until.is_none() && step.for_ticks.is_none() {
                step.until = steps[k..].iter().find_map(is_move).map(Cond::InRange);
            }
            out.steps.push(step);
        }
        for (k, r) in v["rules"].as_array().into_iter().flatten().enumerate() {
            let at = |e: String| format!("rule {}: {e}", k + 1);
            let c = cond(&r["if"], None)
                .map_err(at)?
                .ok_or_else(|| at("no `if`".into()))?;
            let goto = to(&r["goto"], "goto")
                .map_err(at)?
                .ok_or_else(|| at("no `goto`".into()))?;
            out.rules.push(Rule { cond: c, goto });
        }
        Ok(out)
    }

    /// The plan as the replay's viewer lists it: each step's card and the
    /// few words that say when it ends or where it leads.
    pub fn listing(&self, deck: &[Card]) -> Vec<Value> {
        let to = |t: To| match t {
            To::Step(k) => (k + 1).to_string(),
            To::End => "end".into(),
        };
        self.steps
            .iter()
            .map(|s| {
                let mut notes = Vec::new();
                if let Some(c) = s.gate {
                    notes.push(format!("if {}", c.text()));
                }
                if let Some(c) = s.until {
                    notes.push(match c {
                        Cond::InRange(card) => format!("until in range of {}", deck[card].id),
                        c => format!("until {}", c.text()),
                    });
                }
                if let Some(t) = s.for_ticks {
                    notes.push(format!("{} ms", t * 1000 / 60));
                }
                for (name, t) in [
                    ("hit", s.hit),
                    ("blocked", s.blocked),
                    ("missed", s.missed),
                    ("else", s.otherwise),
                    ("then", s.next),
                ] {
                    if let Some(t) = t {
                        notes.push(format!("{name} {}", to(t)));
                    }
                }
                serde_json::json!({ "card": deck[s.card].id, "note": notes.join(", ") })
            })
            .collect()
    }
}

/// The most rules a book has, and the most cards a rule plays.
pub const MAX_RULES: usize = 16;
pub const MAX_CARDS: usize = 5;

/// One rule of a book: when all of `when` holds, do `cards`.
#[derive(Clone, Debug, PartialEq)]
pub struct Law {
    pub when: Vec<Cond>,
    /// Cards of the deck, by their place in it. A stance alone: held while
    /// the rule is the first that holds. Moves: played in a row, each next
    /// one only if the one before hit (a combo).
    pub cards: Vec<usize>,
    /// The last input of a single move is held this long (a charge).
    pub hold_ticks: Option<u32>,
    /// The rule does not fire again before this long.
    pub cooldown: u32,
}

/// A fighter's brain, written by its player: rules read top to bottom 60
/// times a second, the first whose conditions all hold decides what the
/// fighter does.
///
/// ```json
/// {"rules": [
///    {"when": ["he_attacks", "distance<40"], "do": ["block"]},
///    {"when": ["he_is_recovering", "in_range"], "do": ["combo_1", "combo_2", "rasenrengan"]},
///    {"when": ["distance>120", "power<1000"], "do": ["power_charge"], "hold_ms": 1500},
///    {"when": ["power>=2000", "he_is_down"], "do": ["oodama_rasengan"], "cooldown_ms": 3000},
///    {"do": ["approach"]}],
///  "say": "guard his pressure, punish, charge when far"}
/// ```
///
/// `in_range` with no card is of the rule's first move. A rule with no
/// `when` always holds (put it last). While a rule's moves play, no other
/// rule is read; a stance gives way the tick a rule above it holds.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Book {
    pub rules: Vec<Law>,
}

impl Book {
    pub fn parse(v: &Value, deck: &[Card]) -> Result<Self, String> {
        let rules = v["rules"]
            .as_array()
            .filter(|r| !r.is_empty())
            .ok_or("no `rules`: a brain is {\"rules\":[{\"when\":[...],\"do\":[...]},...]}")?;
        if rules.len() > MAX_RULES {
            return Err(format!("{} rules: at most {MAX_RULES}", rules.len()));
        }
        let mut out = Book::default();
        for (k, r) in rules.iter().enumerate() {
            let at = |e: String| format!("rule {}: {e}", k + 1);
            // `do` may be one card or a list
            let ids: Vec<&str> = match &r["do"] {
                Value::String(s) => vec![s.as_str()],
                Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            if ids.is_empty() || ids.len() > MAX_CARDS {
                return Err(at(format!("`do` is 1 to {MAX_CARDS} cards")));
            }
            let mut cards = Vec::new();
            for id in &ids {
                cards.push(
                    deck.iter()
                        .position(|c| c.id == *id)
                        .ok_or_else(|| at(format!("no card `{id}` in the deck")))?,
                );
            }
            let stances = cards.iter().filter(|c| deck[**c].kind == "stance").count();
            if stances > 0 && cards.len() > 1 {
                return Err(at(
                    "a stance is a rule of its own (`do` with that one card)".into(),
                ));
            }
            let first_move = cards.iter().copied().find(|c| deck[*c].kind != "stance");
            let texts: Vec<&str> = match &r["when"] {
                Value::Null => Vec::new(),
                Value::String(s) => vec![s.as_str()],
                Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
                other => return Err(at(format!("`when` is a list of conditions, not {other}"))),
            };
            let mut when = Vec::new();
            for text in texts {
                let t: String = text.chars().filter(|c| !c.is_whitespace()).collect();
                let t = t.to_ascii_lowercase();
                let cond = match t.strip_prefix("in_range") {
                    None => Cond::parse(text).map_err(at)?,
                    Some(rest) => Cond::InRange(match rest.strip_prefix(':') {
                        Some(id) => deck
                            .iter()
                            .position(|c| c.id == id)
                            .ok_or_else(|| at(format!("`{text}`: no card `{id}` in the deck")))?,
                        None => first_move.ok_or_else(|| {
                            at(format!("`{text}`: of which move? write in_range:<card id>"))
                        })?,
                    }),
                };
                if cond == Cond::IAmHit {
                    return Err(at(
                        "`i_am_hit` is not for a brain: a fighter that is hit cannot act".into(),
                    ));
                }
                when.push(cond);
            }
            let ms = |key: &str| -> Result<Option<u32>, String> {
                match &r[key] {
                    Value::Null => Ok(None),
                    v => v
                        .as_f64()
                        .map(|ms| Some((ms.max(0.0) * 0.06) as u32))
                        .ok_or_else(|| at(format!("`{key}` is a number"))),
                }
            };
            // a charge is held without being asked to: for as long as its
            // rule is the first that holds
            let charge = cards.len() == 1 && deck[cards[0]].kind == "charge";
            let hold = ms("hold_ms")?.or(charge.then_some(u32::MAX));
            out.rules.push(Law {
                when,
                cards,
                hold_ticks: hold,
                cooldown: ms("cooldown_ms")?.unwrap_or(0),
            });
        }
        Ok(out)
    }

    /// The moves of a rule as a plan: each next one on a hit of the one
    /// before, anything else ends it.
    pub fn sequence(&self, rule: usize) -> Plan {
        let law = &self.rules[rule];
        let n = law.cards.len();
        Plan {
            steps: law
                .cards
                .iter()
                .enumerate()
                .map(|(k, &card)| Step {
                    card,
                    until: None,
                    for_ticks: law.hold_ticks.filter(|_| n == 1),
                    gate: None,
                    next: Some(To::End),
                    hit: Some(if k + 1 < n { To::Step(k + 1) } else { To::End }),
                    blocked: Some(To::End),
                    missed: Some(To::End),
                    otherwise: None,
                })
                .collect(),
            rules: Vec::new(),
        }
    }

    /// The book as the replay's viewer lists it: per rule, its first card
    /// (for the color) and the rule in words.
    pub fn listing(&self, deck: &[Card]) -> Vec<Value> {
        self.rules
            .iter()
            .map(|law| {
                let when: Vec<String> = law
                    .when
                    .iter()
                    .map(|c| match c {
                        Cond::InRange(card) => format!("in range of {}", deck[*card].id),
                        c => c.text(),
                    })
                    .collect();
                let cards: Vec<&str> = law.cards.iter().map(|c| deck[*c].id.as_str()).collect();
                let mut note = if when.is_empty() {
                    "always".to_string()
                } else {
                    format!("when {}", when.join(" and "))
                };
                if cards.len() > 1 {
                    note.push_str(&format!(", then {}", cards[1..].join(" > ")));
                }
                match law.hold_ticks {
                    Some(u32::MAX) => note.push_str(", held"),
                    Some(t) => note.push_str(&format!(", held {} ms", t * 1000 / 60)),
                    None => {}
                }
                serde_json::json!({ "card": cards[0], "note": note })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::{Needs, Play, Stance};
    use serde_json::json;

    fn deck() -> Vec<Card> {
        let card = |id: &str, play: Play| Card {
            id: id.into(),
            label: id.into(),
            kind: if matches!(play, Play::Stance(_)) {
                "stance"
            } else {
                "attack"
            },
            reach: None,
            facts: Default::default(),
            states: Vec::new(),
            names: Vec::new(),
            power: 0,
            needs: Needs::Any,
            play,
        };
        vec![
            card("approach", Play::Stance(Stance::Approach)),
            card("block", Play::Stance(Stance::Block)),
            card("punch", Play::Move(Vec::new())),
        ]
    }

    #[test]
    fn conditions_parse() {
        assert_eq!(Cond::parse("distance < 45"), Ok(Cond::DistanceLt(45)));
        assert_eq!(Cond::parse("Distance>=80"), Ok(Cond::DistanceGt(80)));
        assert_eq!(Cond::parse("power>=1000"), Ok(Cond::PowerGe(1000)));
        assert_eq!(Cond::parse("life<30%"), Ok(Cond::LifeLt(30)));
        assert_eq!(Cond::parse("he_jumps"), Ok(Cond::HeJumps));
        assert!(Cond::parse("he_is_scared").is_err());
        assert!(Cond::parse("distance<close").is_err());
        let v = View {
            distance: 40,
            power: 500,
            in_range: vec![false, true],
            ..Default::default()
        };
        assert!(!Cond::InRange(0).holds(&v) && Cond::InRange(1).holds(&v));
        assert!(Cond::InRange(7).holds(&v), "no measure: in range");
        assert_eq!(Cond::parse("he_is_above"), Ok(Cond::HeIsAbove));
        assert_eq!(
            Cond::parse("projectile_incoming"),
            Ok(Cond::ProjectileIncoming)
        );
        assert!(Cond::DistanceLt(45).holds(&v) && !Cond::PowerGe(1000).holds(&v));
        for text in ["distance<45", "power>=1000", "life<30%", "he_is_down"] {
            assert_eq!(Cond::parse(text).unwrap().text(), text);
        }
    }

    #[test]
    fn a_plan_parses_with_its_branches() {
        let p = Plan::parse(
            &json!({
                "steps": [
                    {"card": "approach", "until": "distance<45"},
                    {"card": "punch", "hit": 2, "blocked": 3, "missed": "end"},
                    {"card": "block", "for_ms": 500, "next": 1},
                ],
                "rules": [{"if": "i_am_hit", "goto": 3}],
            }),
            &deck(),
        )
        .unwrap();
        assert_eq!(p.steps.len(), 3);
        assert_eq!(p.steps[0].until, Some(Cond::DistanceLt(45)));
        assert_eq!(p.steps[1].hit, Some(To::Step(1)));
        assert_eq!(p.steps[1].missed, Some(To::End));
        assert_eq!(
            (p.steps[2].for_ticks, p.steps[2].next),
            (Some(30), Some(To::Step(0)))
        );
        assert_eq!(
            p.rules,
            [Rule {
                cond: Cond::IAmHit,
                goto: To::Step(2)
            }]
        );
        let l = p.listing(&deck());
        assert_eq!(l[0]["note"], "until distance<45");
        assert_eq!(l[1]["note"], "hit 2, blocked 3, missed end");
    }

    #[test]
    fn in_range_is_of_a_move_of_the_plan() {
        let p = Plan::parse(
            &json!({
                "steps": [
                    {"card": "approach"},
                    {"card": "block", "until": "in_range"},
                    {"card": "punch", "if": "in_range", "else": 1},
                ],
                "rules": [{"if": "in_range:punch", "goto": 3}],
            }),
            &deck(),
        )
        .unwrap();
        // the punch is card 2 of the deck
        assert_eq!(p.steps[0].until, Some(Cond::InRange(2)), "a bare approach");
        assert_eq!(p.steps[1].until, Some(Cond::InRange(2)), "the next move");
        assert_eq!(p.steps[2].gate, Some(Cond::InRange(2)), "its own move");
        assert_eq!(p.rules[0].cond, Cond::InRange(2));
        assert_eq!(p.listing(&deck())[0]["note"], "until in range of punch");
        let bad = |v: Value| Plan::parse(&v, &deck()).unwrap_err();
        assert!(
            bad(json!({"steps": [{"card": "block", "until": "in_range"}]})).contains("which move")
        );
        assert!(
            bad(json!({"steps": [{"card": "block"}], "rules": [{"if": "in_range", "goto": 1}]}))
                .contains("which move")
        );
        assert!(
            bad(json!({"steps": [{"card": "block", "until": "in_range:kick"}]}))
                .contains("no card `kick`")
        );
    }

    #[test]
    fn a_brain_parses_rule_by_rule() {
        let b = Book::parse(
            &json!({"rules": [
                {"when": ["he_attacks", "distance<40"], "do": "block"},
                {"when": ["in_range", "power<1000"], "do": ["punch", "punch"], "cooldown_ms": 500},
                {"when": "his_life<30%", "do": ["punch"], "hold_ms": 1000},
                {"do": ["approach"]},
            ]}),
            &deck(),
        )
        .unwrap();
        assert_eq!(b.rules.len(), 4);
        assert_eq!(b.rules[0].when, [Cond::HeAttacks, Cond::DistanceLt(40)]);
        assert_eq!(b.rules[1].when, [Cond::InRange(2), Cond::PowerLt(1000)]);
        assert_eq!((b.rules[1].cards.len(), b.rules[1].cooldown), (2, 30));
        assert_eq!(b.rules[2].when, [Cond::HisLifeLt(30)]);
        assert!(b.rules[3].when.is_empty());
        // two punches: the second only on a hit of the first
        let seq = b.sequence(1);
        assert_eq!(
            (seq.steps[0].hit, seq.steps[0].missed),
            (Some(To::Step(1)), Some(To::End))
        );
        assert_eq!(seq.steps[1].hit, Some(To::End));
        assert_eq!(
            b.sequence(2).steps[0].for_ticks,
            Some(60),
            "a single move is held"
        );
        let l = b.listing(&deck());
        assert_eq!(l[0]["note"], "when he_attacks and distance<40");
        assert_eq!(
            l[1]["note"],
            "when in range of punch and power<1000, then punch"
        );
        assert_eq!(l[3]["note"], "always");
        let bad = |v: Value| Book::parse(&v, &deck()).unwrap_err();
        assert!(bad(json!({})).contains("rules"));
        assert!(bad(json!({"rules": [{"do": ["kick"]}]})).contains("no card `kick`"));
        assert!(bad(json!({"rules": [{"do": ["block", "punch"]}]})).contains("stance"));
        assert!(
            bad(json!({"rules": [{"when": ["in_range"], "do": ["block"]}]})).contains("which move")
        );
        assert!(
            bad(json!({"rules": [{"when": ["i_am_hit"], "do": ["block"]}]})).contains("cannot act")
        );
        assert!(bad(json!({"rules": [{"when": ["sleepy"], "do": ["block"]}]})).contains("rule 1"));
    }

    #[test]
    fn a_bad_plan_is_refused_with_the_reason() {
        let bad = |v: Value| Plan::parse(&v, &deck()).unwrap_err();
        assert!(bad(json!({})).contains("steps"));
        assert!(bad(json!({"steps": [{"card": "fireball"}]})).contains("no card `fireball`"));
        assert!(bad(json!({"steps": [{"card": "punch", "hit": 4}]})).contains("not a step"));
        assert!(bad(json!({"steps": [{"card": "block", "until": "bored"}]})).contains("step 1"));
        assert!(bad(json!({"steps": [{"card": "block", "until": "i_am_hit"}]})).contains("rules"));
        assert!(
            bad(json!({"steps": [{"card": "block"}], "rules": [{"if": "he_jumps"}]}))
                .contains("goto")
        );
        let eleven: Vec<Value> = (0..11).map(|_| json!({"card": "block"})).collect();
        assert!(bad(json!({ "steps": eleven })).contains("at most"));
    }
}
