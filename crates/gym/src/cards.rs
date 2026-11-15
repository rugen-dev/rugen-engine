//! A fighter's deck: the actions a player picks from instead of typing
//! inputs, so every player stands on the same ground and what is compared is
//! the choice and its timing.
//!
//! Two kinds of cards:
//! - **stances**, the same for every fighter (approach, back off, block...):
//!   held until another stance is chosen, so a slow player's fighter keeps
//!   doing what it was told;
//! - **moves**, read from the fighter's own `.cmd`: every `[State -1, Label]`
//!   rule that changes state on a command becomes a card named by its label,
//!   played once (its command motion), after which the stance resumes. The
//!   state it leads to (and the few numbered right after it, where authors
//!   put a move's follow-ups) says what it is: an attack (a `HitDef`), a
//!   projectile, a special (a `Helper` does the hitting), a super (any of
//!   those for 1000 power or more), a boost (nothing hits but it costs
//!   power or freezes the screen: a transformation, a charge, a buff), else
//!   a plain move (a run, a dash).
//!
//! Rules with the same input in the same posture are one card: a fighter
//! with forms (Goku's transformations) gives a different move for the same
//! buttons in each, and which one comes out is its business, not the
//! player's.
//!
//! The rules are read as text, by their first route (`triggerall` and
//! `trigger1`): the command, the power it costs and the posture it needs.
//! That is a reading of the common way fighters are written, not an
//! evaluation: an unusual rule gives a card that may not come out.

use std::path::Path;

use rugen_sim::CharData;
use rugen_sim::command::CommandDef;
use serde_json::{Value, json};

use crate::{Rel, parse_plan, plan_of};

/// What a fighter holds while no move is playing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stance {
    #[default]
    Wait,
    Approach,
    BackOff,
    Block,
    BlockLow,
    Crouch,
}

impl Stance {
    /// This tick's input, the other fighter `distance` stage units away.
    pub fn rel(self, distance: i32) -> Rel {
        let mut r = Rel::default();
        match self {
            Stance::Wait => {}
            // walks until the bodies nearly touch
            Stance::Approach => r.f = distance > 4,
            Stance::BackOff | Stance::Block => r.b = true,
            Stance::BlockLow => (r.d, r.b) = (true, true),
            Stance::Crouch => r.d = true,
        }
        r
    }
}

/// The posture a move needs to come out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Needs {
    Any,
    Ground,
    Air,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Play {
    Stance(Stance),
    /// The ticks of the move's inputs.
    Move(Vec<Rel>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    /// What a player answers with.
    pub id: String,
    /// The rule's label in the `.cmd` (or the stance's description).
    pub label: String,
    /// `stance`, `attack`, `projectile`, `special`, `super`, `boost`, `move`.
    pub kind: &'static str,
    /// The states a move leads to, when its rules name them by number: the
    /// fighter entering one is the move coming out. Several when the same
    /// input does different things by the fighter's form or mode.
    pub states: Vec<i32>,
    /// Each of those states with the label of the rule that leads to it:
    /// what the move is called when it is seen on screen.
    pub names: Vec<(i32, String)>,
    /// How far the move's hitbox reaches ahead of the fighter's axis, in
    /// the fighter's own units: the widest attack box of the animations
    /// its states start with. `None` when it has none (a projectile, a
    /// move whose hit comes from a helper, a plain move).
    pub reach: Option<i32>,
    /// Its timing and damage, as far as its files say ([`Facts`]).
    pub facts: Facts,
    /// Power the move costs (0: none).
    pub power: i32,
    pub needs: Needs,
    pub play: Play,
}

/// What a move's files say of it, each when it can be read: ticks before
/// its first attack box, ticks its animation lasts, and the damage of its
/// (first) hit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub startup: Option<i32>,
    pub lasts: Option<i32>,
    pub damage: Option<i32>,
}

impl Card {
    pub fn json(&self) -> Value {
        let mut v = json!({ "id": self.id, "label": self.label, "kind": self.kind });
        // times in milliseconds, as a player reads them
        let ms = |ticks: i32| ticks * 1000 / 60;
        if let Some(t) = self.facts.startup {
            v["startup_ms"] = ms(t).into();
        }
        if let Some(t) = self.facts.lasts {
            v["lasts_ms"] = ms(t).into();
        }
        if let Some(d) = self.facts.damage {
            v["damage"] = d.into();
        }
        if self.power > 0 {
            v["power"] = self.power.into();
        }
        match self.needs {
            Needs::Any => {}
            Needs::Ground => v["needs"] = "ground".into(),
            Needs::Air => v["needs"] = "air".into(),
        }
        v
    }

    /// The fighter can play it now, as far as the card knows: enough
    /// power, and in the air or on the ground as the move needs.
    pub fn ready(&self, power: i32, airborne: bool) -> bool {
        power >= self.power
            && match self.needs {
                Needs::Any => true,
                Needs::Ground => !airborne,
                Needs::Air => airborne,
            }
    }
}

fn common(step: u32) -> Vec<Card> {
    let stance = |id: &str, label: &str, s: Stance| Card {
        id: id.into(),
        label: label.into(),
        kind: "stance",
        states: Vec::new(),
        names: Vec::new(),
        reach: None,
        facts: Facts::default(),
        power: 0,
        needs: Needs::Any,
        play: Play::Stance(s),
    };
    let jump = |id: &str, label: &str, plan: &str| Card {
        id: id.into(),
        label: label.into(),
        kind: "move",
        states: Vec::new(),
        names: Vec::new(),
        reach: None,
        facts: Facts::default(),
        power: 0,
        needs: Needs::Ground,
        play: Play::Move(parse_plan(plan, step, 90).unwrap_or_default()),
    };
    vec![
        stance("wait", "stand still", Stance::Wait),
        stance(
            "approach",
            "walk toward the opponent, stop at arm's length",
            Stance::Approach,
        ),
        stance(
            "back_off",
            "walk away (guards high when attacked)",
            Stance::BackOff,
        ),
        stance(
            "block",
            "guard standing: stops high and mid attacks",
            Stance::Block,
        ),
        stance(
            "block_low",
            "guard crouching: stops low and mid attacks",
            Stance::BlockLow,
        ),
        stance("crouch", "crouch without guarding", Stance::Crouch),
        jump("jump", "jump straight up", "U*4"),
        jump("jump_in", "jump toward the opponent", "UF*4"),
        jump("jump_back", "jump away", "UB*4"),
    ]
}

/// `label` as an id: lowercase words joined by `_`.
fn slug(label: &str) -> String {
    let mut out = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_string()
}

/// One `[State -1, Label]` block of a `.cmd`.
#[derive(Default)]
struct Rule {
    label: String,
    kind: String,
    value: String,
    /// `triggerall` and `trigger1` lines, lowercase.
    route: Vec<String>,
}

fn rules(text: &str) -> Vec<Rule> {
    let mut out: Vec<Rule> = Vec::new();
    let mut in_rule = false;
    for line in text.lines() {
        let line = line.split(';').next().unwrap_or("").trim();
        if let Some(head) = line.strip_prefix('[') {
            let head = head.trim_end_matches(']').trim();
            let lower = head.to_ascii_lowercase();
            in_rule = lower.starts_with("state -1");
            if in_rule {
                let label = head.split_once(',').map_or("", |(_, l)| l.trim());
                out.push(Rule {
                    label: label.to_string(),
                    ..Default::default()
                });
            }
            continue;
        }
        let (Some(rule), Some((key, val))) =
            (out.last_mut().filter(|_| in_rule), line.split_once('='))
        else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let val = val.trim();
        match key.as_str() {
            "type" => rule.kind = val.to_ascii_lowercase(),
            "value" => rule.value = val.to_string(),
            "triggerall" | "trigger1" => rule.route.push(val.to_ascii_lowercase()),
            _ => {}
        }
    }
    out
}

/// The names in `command = "name"` tests of `line` (not `!=`).
fn commands(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(at) = rest.find("command") {
        rest = &rest[at + 7..];
        let after = rest.trim_start();
        if let Some(q) = after.strip_prefix('=').map(str::trim_start)
            && let Some(name) = q.strip_prefix('"').and_then(|n| n.split('"').next())
        {
            out.push(name);
        }
    }
    out
}

/// The number after `key >=` / `key >` in `line`.
fn at_least(line: &str, key: &str) -> Option<i32> {
    let at = line.find(key)?;
    let rest = line[at + key.len()..].trim_start();
    let rest = rest.strip_prefix(">=").or_else(|| rest.strip_prefix('>'))?;
    let digits: String = rest
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn needs(route: &[String]) -> (Needs, bool) {
    let has = |what: &str| {
        route
            .iter()
            .any(|l| l.replace(' ', "").contains(&what.replace(' ', "")))
    };
    if has("statetype = a") {
        (Needs::Air, false)
    } else if has("statetype = c") {
        (Needs::Ground, true)
    } else if has("statetype = s") || has("statetype != a") {
        (Needs::Ground, false)
    } else {
        (Needs::Any, false)
    }
}

/// What the state a rule leads to is. A move often goes on in the states
/// numbered after its first (3000, 3001, 3002...): the next few count too.
fn kind_of(data: &CharData, state: Option<i32>, power: i32) -> &'static str {
    let has = |k: &str| {
        state.is_some_and(|s| {
            data.states
                .range(s..s.saturating_add(10))
                .any(|(_, st)| st.ctrls.iter().any(|c| c.kind.eq_ignore_ascii_case(k)))
        })
    };
    let hits = has("hitdef") || has("projectile") || has("helper");
    if hits && power >= 1000 {
        "super"
    } else if has("projectile") {
        "projectile"
    } else if has("hitdef") {
        "attack"
    } else if has("helper") {
        "special"
    } else if power > 0 || has("superpause") {
        "boost"
    } else {
        "move"
    }
}

/// How far ahead of the axis the attack boxes of a state's animation go
/// (and of the few states numbered after it, where a move goes on).
fn reach_of(data: &CharData, state: Option<i32>) -> Option<i32> {
    let s = state?;
    data.states
        .range(s..s.saturating_add(4))
        .filter_map(|(_, st)| match st.def.anim.as_ref()?.ops.as_slice() {
            [rugen_cns::code::Op::Int(n)] => data.air.get(*n),
            _ => None,
        })
        .flat_map(|a| a.frames.iter())
        .flat_map(|f| f.clsn1.iter())
        .map(|b| b[0].max(b[2]))
        .max()
        .filter(|r| *r > 0)
}

/// A move's timing (from the animation its state starts with) and damage
/// (from the first `HitDef` of its states that gives a plain number).
fn facts_of(data: &CharData, state: Option<i32>) -> Facts {
    let Some(s) = state else {
        return Facts::default();
    };
    let literal = |code: &rugen_cns::Code| match code.ops.as_slice() {
        [rugen_cns::code::Op::Int(n)] => Some(*n),
        _ => None,
    };
    let anim = data
        .states
        .get(&s)
        .and_then(|st| literal(st.def.anim.as_ref()?))
        .and_then(|n| data.air.get(n));
    let (mut startup, mut lasts) = (None, None);
    if let Some(a) = anim {
        let time = |f: &rugen_formats::air::Frame| f.time.max(0);
        if let Some(first) = a.frames.iter().position(|f| !f.clsn1.is_empty()) {
            startup = Some(a.frames[..first].iter().map(time).sum());
        }
        // an animation that ends on a held frame has no length of its own
        if a.frames.iter().all(|f| f.time >= 0) {
            lasts = Some(a.frames.iter().map(time).sum());
        }
    }
    let damage = data
        .states
        .range(s..s.saturating_add(4))
        .flat_map(|(_, st)| st.ctrls.iter())
        .find_map(|c| match &c.op {
            rugen_sim::sctrl::Sctrl::HitDef(h) => {
                literal(h.exprs.get("damage")?.1.first()?).filter(|d| *d > 0)
            }
            _ => None,
        });
    Facts {
        startup,
        lasts,
        damage,
    }
}

/// The move cards of a `.cmd`'s text.
fn moves(text: &str, data: &CharData, defs: &[CommandDef], step: u32) -> Vec<Card> {
    let mut out: Vec<Card> = Vec::new();
    for rule in rules(text) {
        if rule.kind != "changestate" || rule.label.is_empty() {
            continue;
        }
        let names: Vec<&str> = rule.route.iter().flat_map(|l| commands(l)).collect();
        let held = |n: &str| matches!(n, "holdfwd" | "holdback" | "holdup" | "holddown");
        let Some(def) = names
            .iter()
            .find(|n| !held(n))
            .and_then(|n| defs.iter().find(|d| d.name.eq_ignore_ascii_case(n)))
        else {
            continue;
        };
        let (needs, crouch) = needs(&rule.route);
        // directions the rule wants held with the command
        let mut hold = String::new();
        for (name, dir) in [
            ("holddown", "D"),
            ("holdfwd", "F"),
            ("holdback", "B"),
            ("holdup", "U"),
        ] {
            if names.contains(&name) || (dir == "D" && crouch) {
                hold.push_str(dir);
                hold.push('+');
            }
        }
        let motion = plan_of(def);
        let plan = if hold.is_empty() {
            motion
        } else {
            let steps: Vec<String> = motion.split(',').map(|s| format!("{hold}{s}")).collect();
            format!("{}*3,{}", hold.trim_end_matches('+'), steps.join(","))
        };
        let Ok(ticks) = parse_plan(&plan, step, 90) else {
            continue;
        };
        let power = rule
            .route
            .iter()
            .filter_map(|l| at_least(l, "power"))
            .max()
            .unwrap_or(0);
        let state: Option<i32> = rule.value.parse().ok();
        // the same input in the same posture is one card, whatever the
        // rule: which state it gives is the fighter's business (its form,
        // a mode, a combo). The card carries every name and every state.
        let play = Play::Move(ticks);
        if let Some(same) = out.iter_mut().find(|c| c.play == play && c.needs == needs) {
            same.states.extend(state);
            // the same input in another form: the shortest reach is the
            // one that holds in every form
            same.reach = match (same.reach, reach_of(data, state)) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            same.names.extend(state.map(|st| (st, rule.label.clone())));
            same.power = same.power.min(power);
            if !same
                .label
                .split(" / ")
                .any(|l| l.eq_ignore_ascii_case(&rule.label))
            {
                match same.label.matches(" / ").count() {
                    0 | 1 => same.label = format!("{} / {}", same.label, rule.label),
                    2 => same.label.push_str(" / ..."),
                    _ => {}
                }
            }
            continue;
        }
        let id = slug(&rule.label);
        if id.is_empty() || out.iter().any(|c| c.id == id) {
            continue;
        }
        out.push(Card {
            id,
            names: state
                .map(|st| (st, rule.label.clone()))
                .into_iter()
                .collect(),
            label: rule.label,
            reach: reach_of(data, state),
            facts: facts_of(data, state),
            kind: kind_of(data, state, power),
            states: state.into_iter().collect(),
            power,
            needs,
            play,
        });
        // a fighter with more rules than this is not a deck any more
        if out.len() >= 150 {
            break;
        }
    }
    out
}

/// A fighter's deck: the common cards, then its own moves in the order of
/// its `.cmd`. A fighter whose `.cmd` cannot be read keeps the common ones.
pub fn deck(def_path: &Path, data: &CharData, defs: &[CommandDef], step: u32) -> Vec<Card> {
    let mut cards = common(step);
    let text = rugen_formats::def::Def::open(def_path)
        .ok()
        .and_then(|def| {
            let name = def.section("files")?.raw("cmd")?.to_string();
            let dir = def_path.parent().unwrap_or(Path::new("."));
            let path = rugen_formats::vfs::resolve(dir, &name)?;
            Some(rugen_formats::text::decode(
                &rugen_formats::vfs::read(&path).ok()?,
            ))
        });
    if let Some(text) = text {
        for card in moves(&text, data, defs, step) {
            if !cards.iter().any(|c| c.id == card.id) {
                cards.push(card);
            }
        }
    }
    cards
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_read_by_their_first_route() {
        let text = r#"
[Command]
name = "a"
[Statedef -1]
[State -1, Triple Kung Fu Palm]   ; a super
type = ChangeState
value = 3000
triggerall = command = "TripleKFPalm"
triggerall = power >= 1000
trigger1 = statetype = S
trigger1 = ctrl
trigger2 = statetype != A
[State -1, Crouching Light Kick]
type = ChangeState
value = 430
triggerall = command = "a"
triggerall = command = "holddown"
trigger1 = statetype = C
[State -1, Combo condition Reset]
type = VarSet
trigger1 = 1
"#;
        let r = rules(text);
        assert_eq!(r.len(), 3);
        assert_eq!(
            (r[0].label.as_str(), r[0].value.as_str()),
            ("Triple Kung Fu Palm", "3000")
        );
        assert_eq!(r[0].route.len(), 4, "trigger2 is another route");
        assert_eq!(commands(&r[0].route[0]), ["triplekfpalm"]);
        assert_eq!(at_least(&r[0].route[1], "power"), Some(1000));
        assert_eq!(needs(&r[0].route), (Needs::Ground, false));
        assert_eq!(needs(&r[1].route), (Needs::Ground, true));
        assert_eq!(r[2].kind, "varset");
        assert!(commands("command != \"x\" && command = \"y\"") == ["y"]);
    }

    #[test]
    fn labels_become_ids() {
        assert_eq!(slug("Kung Fu Throw"), "kung_fu_throw");
        assert_eq!(
            slug("Smash Kung Fu Upper (uses one super bar)"),
            "smash_kung_fu_upper_uses_one_super_bar"
        );
        assert_eq!(slug("--"), "");
    }

    #[test]
    fn stances_hold_their_direction() {
        assert!(Stance::Approach.rel(100).f && !Stance::Approach.rel(2).f);
        assert!(Stance::Block.rel(50).b && !Stance::Block.rel(50).d);
        assert!(Stance::BlockLow.rel(50).b && Stance::BlockLow.rel(50).d);
        assert_eq!(Stance::Wait.rel(50), Rel::default());
    }
}
