//! avalon-sim — the pure deterministic core (RFC-AVL-001 DR-7).
//!
//! No tokio, no IO, no wall clocks, no Ollama. Money is integer pence, fuel is
//! millilitres, RNG is seeded ChaCha8 carried inside the state so replays are
//! bit-exact: `(Mission, Command, &ContentDb) -> (Mission, Vec<Event>)`.
//!
//! The game is a roguelike run generator: each `Mission` is self-contained —
//! roll a terrain, a cargo, a destination, a cast; drive the legs; win by
//! delivering or lose by getting caught, breaking down, or running dry. Tuned
//! so a sensible playthrough loses ~60% of the time (see the `loss_rate` test).
//! Nothing persists between missions.
//!
//! The persistent-world systems (NPC memory, gossip, The Inquiry) are shelved
//! under `src/_shelved/` — out of the build, kept for a future mode.

mod engine;

pub use avalon_content::ContentDb;

use avalon_content::Storylet;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};

/// Cobb's fixed aptitudes. A d12 + skill beats the test's DC.
pub fn skill_value(name: &str) -> i64 {
    match name {
        "paperwork" => 2,
        "spanners" => 1,
        "charm" => 1,
        _ => 0,
    }
}

pub const MAX_WEAR: i64 = 10;
pub const MAX_HEAT: i64 = 10;
/// Pence a roadside farm extorts for an emergency 8 L when you run dry.
pub const JERRY_CAN_PENCE: i64 = 360;
pub const JERRY_CAN_ML: i64 = 8_000;
pub const DIESEL_BUY_L: i64 = 12;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum Stage {
    /// The depot: meet the fixer, top up fuel, set off.
    Briefing,
    /// On the road, facing `beats[idx]`.
    Beat { idx: usize },
    /// The aftermath of a beat choice, shown before the road continues.
    Outcome { idx: usize, text: String },
    /// The drop.
    Arrival,
    /// Terminal. `outcome` holds the verdict and the summary lines.
    Over,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    InProgress,
    Won { haul_pence: i64, lines: Vec<String> },
    Lost { reason: String, lines: Vec<String> },
}

impl Outcome {
    pub fn is_over(&self) -> bool {
        !matches!(self, Outcome::InProgress)
    }
}

/// One self-contained smuggling run. Ephemeral: built fresh per session, never
/// persisted, gone when the tab closes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Mission {
    pub seed: u64,
    // --- the job ---
    pub terrain: String,
    pub terrain_name: String,
    pub terrain_flavor: String,
    pub cargo: String,
    pub qty_desc: String,
    pub dest_name: String,
    pub illicit: bool,
    /// The haul on success — the score. Not spending money.
    pub reward_pence: i64,
    /// NPC ids: who briefs you, who's hunting you this run.
    pub fixer: String,
    pub antagonist: String,
    // --- clocks ---
    /// Spending money for fuel and bribes during the run.
    pub cash_pence: i64,
    pub diesel_price_pence: i64,
    pub fuel_ml: i64,
    pub max_fuel_ml: i64,
    /// Per-leg burn for this terrain.
    pub leg_fuel_ml: i64,
    /// Truck strain. MAX_WEAR = breakdown, mission lost.
    pub wear: i64,
    /// Pursuit. MAX_HEAT = caught, mission lost.
    pub heat: i64,
    pub dc_mod: i64,
    pub heat_per_leg: i64,
    pub wear_per_leg: i64,
    // --- structure ---
    pub legs_total: usize,
    /// Legs driven so far (for the route map).
    pub leg: usize,
    pub beats: Vec<String>,
    pub stage: Stage,
    pub outcome: Outcome,
    pub flags: BTreeSet<String>,
    pub talked: bool,
    pub cargo_lost: bool,
    pub rng: ChaCha8Rng,
}

impl Mission {
    /// Roll a fresh mission from a seed. The daemon supplies a wall-clock seed;
    /// the sim stays pure.
    pub fn new(seed: u64, content: &ContentDb) -> Self {
        engine::generate(seed, content)
    }

    pub fn fuel_l(&self) -> i64 {
        self.fuel_ml / 1_000
    }

    pub fn fuel_pct(&self) -> i64 {
        if self.max_fuel_ml <= 0 {
            0
        } else {
            (self.fuel_ml * 100 / self.max_fuel_ml).clamp(0, 100)
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Take choice `idx` of the current scene. Disabled/out-of-range = no-op.
    Choose { idx: usize },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Narration { text: String },
}

/// What the player sees. Pure function of state + content; never touches RNG.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Scene {
    pub title: String,
    pub body: Vec<String>,
    pub choices: Vec<Choice>,
    /// NPC id whose portrait fronts this scene, if any (UI hint).
    pub speaker: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Choice {
    pub label: String,
    pub enabled: bool,
    pub note: Option<String>,
}

impl Choice {
    pub(crate) fn on(label: impl Into<String>) -> Self {
        Self { label: label.into(), enabled: true, note: None }
    }
    pub(crate) fn off(label: impl Into<String>, note: impl Into<String>) -> Self {
        Self { label: label.into(), enabled: false, note: Some(note.into()) }
    }
}

pub fn scene(m: &Mission, content: &ContentDb) -> Scene {
    engine::scene(m, content)
}

/// The single pure transition.
pub fn step(m: &Mission, cmd: &Command, content: &ContentDb) -> (Mission, Vec<Event>) {
    let mut next = m.clone();
    let mut events = Vec::new();
    match cmd {
        Command::Choose { idx } => {
            let current = scene(m, content);
            let valid = current.choices.get(*idx).map(|c| c.enabled).unwrap_or(false);
            if valid && !m.outcome.is_over() {
                engine::choose(&mut next, *idx, content, &mut events);
            }
        }
    }
    (next, events)
}

pub(crate) fn storylet<'c>(content: &'c ContentDb, id: &str) -> Option<&'c Storylet> {
    content.storylets.iter().find(|s| s.id == id)
}

/// Stable hash of the full state, for golden-replay tests.
pub fn mission_hash(m: &Mission) -> u64 {
    let json = serde_json::to_string(m).expect("Mission is always serializable");
    let mut h = DefaultHasher::new();
    json.hash(&mut h);
    h.finish()
}

/// Pre-decimal money: 12d = 1s, 20s = £1.
pub fn fmt_pence(p: i64) -> String {
    let sign = if p < 0 { "-" } else { "" };
    let p = p.abs();
    let (pounds, s, d) = (p / 240, (p % 240) / 12, p % 12);
    match (pounds, s, d) {
        (0, 0, d) => format!("{sign}{d}d"),
        (0, s, 0) => format!("{sign}{s}s"),
        (0, s, d) => format!("{sign}{s}s {d}d"),
        (l, s, 0) => format!("{sign}£{l} {s}s"),
        (l, s, d) => format!("{sign}£{l} {s}s {d}d"),
    }
}

pub(crate) fn narrate(events: &mut Vec<Event>, text: impl Into<String>) {
    events.push(Event::Narration { text: text.into() });
}

/// d12 + skill.
pub(crate) fn roll(rng: &mut ChaCha8Rng, skill: i64) -> i64 {
    rng.gen_range(1..=12) + skill
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn content() -> ContentDb {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        avalon_content::load(&dir).expect("content loads and validates")
    }

    /// A "sensible but not optimal" policy: top up fuel once, set off, then on
    /// the road prefer the lowest-risk option — never smash a gate, never run
    /// an ambush, take inspections over bluffs. This is the baseline the ~60%
    /// loss target is measured against.
    fn sensible_play(seed: u64, c: &ContentDb) -> Outcome {
        let mut m = Mission::new(seed, c);
        let mut guard = 0;
        loop {
            if m.outcome.is_over() {
                return m.outcome;
            }
            let s = scene(&m, c);
            let idx = pick(&m, &s);
            let (next, _) = step(&m, &Command::Choose { idx }, c);
            m = next;
            guard += 1;
            assert!(guard < 200, "mission did not terminate (seed {seed})");
        }
    }

    fn pick(m: &Mission, s: &Scene) -> usize {
        let enabled: Vec<usize> =
            (0..s.choices.len()).filter(|&i| s.choices[i].enabled).collect();
        match &m.stage {
            Stage::Briefing => {
                // top up fuel if affordable and low, else set off (last choice).
                if m.fuel_pct() < 80 && s.choices.get(1).map(|c| c.enabled).unwrap_or(false) {
                    1
                } else {
                    *enabled.last().unwrap()
                }
            }
            // On the road / outcome / arrival: take the FIRST enabled choice —
            // authored so choice 0 is the cautious, lawful option.
            _ => enabled[0],
        }
    }

    #[test]
    fn content_loads_and_validates() {
        let c = content();
        assert!(c.terrains.len() >= 4);
        assert!(c.storylets.len() >= 8);
        assert!(c.npcs.len() >= 5);
    }

    #[test]
    fn golden_replay_is_bit_exact() {
        let c = content();
        let a = sensible_play(0xC0BB, &c);
        let b = sensible_play(0xC0BB, &c);
        assert_eq!(a, b);
    }

    #[test]
    fn missions_vary_by_seed() {
        let c = content();
        let mut terrains = BTreeSet::new();
        let mut cargoes = BTreeSet::new();
        for seed in 0..24u64 {
            let m = Mission::new(seed, &c);
            terrains.insert(m.terrain.clone());
            cargoes.insert(m.cargo.clone());
        }
        assert!(terrains.len() >= 3, "terrains too samey: {terrains:?}");
        assert!(cargoes.len() >= 3, "cargoes too samey: {cargoes:?}");
    }

    #[test]
    fn every_mission_terminates_and_is_decisive() {
        let c = content();
        for seed in 0..200u64 {
            match sensible_play(seed, &c) {
                Outcome::Won { haul_pence, .. } => assert!(haul_pence > 0),
                Outcome::Lost { reason, .. } => assert!(!reason.is_empty()),
                Outcome::InProgress => panic!("seed {seed} ended InProgress"),
            }
        }
    }

    /// The headline balance gate: sensible play should lose roughly 60% of the
    /// time — hard but earned. Band kept wide enough to survive content tweaks.
    #[test]
    fn loss_rate_is_about_sixty_percent() {
        let c = content();
        let n = 600u64;
        let losses = (0..n)
            .filter(|&seed| matches!(sensible_play(seed, &c), Outcome::Lost { .. }))
            .count();
        let pct = losses * 100 / n as usize;
        assert!(
            (45..=72).contains(&pct),
            "loss rate {pct}% outside the 45–72% target band ({losses}/{n})"
        );
    }

    #[test]
    fn invariants_hold() {
        let c = content();
        for seed in 0..200u64 {
            let mut m = Mission::new(seed, &c);
            let mut guard = 0;
            while !m.outcome.is_over() {
                assert!((0..=MAX_WEAR).contains(&m.wear));
                assert!((0..=MAX_HEAT).contains(&m.heat));
                assert!(m.cash_pence >= 0);
                assert!(m.fuel_ml >= 0);
                let s = scene(&m, &c);
                let idx = (0..s.choices.len()).find(|&i| s.choices[i].enabled).unwrap();
                let (next, _) = step(&m, &Command::Choose { idx }, &c);
                m = next;
                guard += 1;
                assert!(guard < 200);
            }
        }
    }

    #[test]
    fn disabled_and_out_of_range_choices_are_noops() {
        let c = content();
        let m = Mission::new(7, &c);
        let before = mission_hash(&m);
        // Briefing choice 0 is "talk" (enabled); pick a wild index.
        let (after, ev) = step(&m, &Command::Choose { idx: 99 }, &c);
        assert_eq!(before, mission_hash(&after));
        assert!(ev.is_empty());
    }
}


