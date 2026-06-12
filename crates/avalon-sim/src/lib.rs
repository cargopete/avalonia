//! avalon-sim — the pure deterministic core (RFC-AVL-001 DR-7).
//!
//! No tokio, no IO, no wall clocks, no Ollama. Time is integer ticks, money is
//! integer pence, fuel is millilitres, RNG is seeded ChaCha8 carried inside the
//! world state so replays are bit-exact:
//! `(WorldState, Command, &ContentDb) -> (WorldState, Vec<Event>)`.
//!
//! Stage 2: beats come from the TOML storylet deck (gates → prose → effects).
//! The scene presented to the player is a pure function of state + content;
//! choices are validated against it before any transition. LLM text enters
//! canon only via recorded-oracle commands the daemon issues after validation.

mod engine;

pub use avalon_content::ContentDb;

use avalon_content::Storylet;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

/// One tick = one phase of the day; five phases per day, Thrushcombe-style.
pub type Tick = u64;

pub const PHASES_PER_DAY: u64 = 5;
pub const PHASE_NAMES: [&str; 5] = ["dawn", "forenoon", "afternoon", "evening", "night"];

/// Cobb's fixed aptitudes (a character sheet can wait).
pub fn skill_value(name: &str) -> i64 {
    match name {
        "paperwork" => 2,
        "spanners" => 1,
        "charm" => 1,
        _ => 0,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Contract {
    pub cargo: String,
    pub qty_desc: String,
    pub dest_id: String,
    pub dest_name: String,
    pub payment_pence: i64,
    pub illicit: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum RunStage {
    /// Wychford depot: talk, buy fuel, accept the contract, depart.
    Town,
    /// On the road, facing `beats[idx]`.
    Beat { idx: usize },
    /// The aftermath of a beat choice, shown before the road continues.
    Outcome { idx: usize, text: String },
    /// The destination yard.
    Arrival,
    /// Ledger Close: what changed, and the door to the next day.
    Summary { lines: Vec<String> },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RunState {
    pub contract: Contract,
    pub accepted: bool,
    pub talked: bool,
    pub stage: RunStage,
    /// Storylet ids drawn at depart; the run's hand of cards.
    pub beats: Vec<String>,
    /// Run-scoped flags; folded into world flags at Ledger Close.
    pub flags: BTreeSet<String>,
    /// Percent multiplier on contract payment at delivery (spoilage, bonuses).
    pub payment_pct: i64,
    // Snapshot at departure, for the Ledger Close delta lines.
    pub start_cash: i64,
    pub start_fuel_ml: i64,
    pub start_wear: i64,
    pub start_suspicion: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WorldState {
    pub seed: u64,
    pub tick: Tick,
    /// Cobb's cash, in pence. Integers only in canon (DR-7). Never negative.
    pub cash_pence: i64,
    /// Bedford TK tank, millilitres.
    pub fuel_ml: i64,
    /// Diesel price per litre in Wychford, pence.
    pub diesel_price_pence: i64,
    /// Clock: 0..=10. The truck's opinion of you.
    pub bedford_wear: i64,
    /// Clock: 0..=10. How interested officialdom has become.
    pub guild_suspicion: i64,
    /// Faction reputation, -5..=5 each: collective, guild, carver.
    pub factions: BTreeMap<String, i64>,
    pub runs_completed: u32,
    /// Persistent world flags (the facts database, embryonic).
    pub flags: BTreeSet<String>,
    /// Carried RNG: replaying the same seed + command log is bit-exact.
    pub rng: ChaCha8Rng,
    pub run: RunState,
}

impl WorldState {
    pub fn new(seed: u64, content: &ContentDb) -> Self {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let contract = engine::gen_contract(&mut rng, content);
        let factions = avalon_content::FACTIONS
            .iter()
            .map(|f| (f.to_string(), 0i64))
            .collect();
        Self {
            seed,
            tick: 0,
            cash_pence: 12_000, // £50: one tank of diesel and a bad reputation
            fuel_ml: 18_000,
            diesel_price_pence: 18,
            bedford_wear: 2,
            guild_suspicion: 0,
            factions,
            runs_completed: 0,
            flags: BTreeSet::new(),
            rng,
            run: RunState {
                contract,
                accepted: false,
                talked: false,
                stage: RunStage::Town,
                beats: Vec::new(),
                flags: BTreeSet::new(),
                payment_pct: 100,
                start_cash: 12_000,
                start_fuel_ml: 18_000,
                start_wear: 2,
                start_suspicion: 0,
            },
        }
    }

    pub fn day(&self) -> u64 {
        self.tick / PHASES_PER_DAY
    }

    pub fn phase_name(&self) -> &'static str {
        PHASE_NAMES[(self.tick % PHASES_PER_DAY) as usize]
    }

    pub fn faction(&self, id: &str) -> i64 {
        self.factions.get(id).copied().unwrap_or(0)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Take choice `idx` of the current scene. Disabled/out-of-range = no-op.
    Choose { idx: usize },
    /// Offline catch-up: `days` real days passed while nobody was driving.
    CatchUp { days: u64 },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Narration {
        tick: Tick,
        text: String,
    },
    PriceDrift {
        tick: Tick,
        good: String,
        settlement: String,
        price_pence: i64,
    },
    /// A canonical, attributable happening — the Inquiry's ground truth.
    Fact {
        tick: Tick,
        fact_id: String,
        actor: String,
        action: String,
        location: String,
    },
}

/// What the player sees. Pure function of state + content; never touches RNG.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Scene {
    pub title: String,
    pub body: Vec<String>,
    pub choices: Vec<Choice>,
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

pub fn scene(state: &WorldState, content: &ContentDb) -> Scene {
    engine::scene(state, content)
}

/// The single pure transition. Everything canonical happens here.
pub fn step(state: &WorldState, cmd: &Command, content: &ContentDb) -> (WorldState, Vec<Event>) {
    let mut next = state.clone();
    let mut events = Vec::new();
    match cmd {
        Command::Choose { idx } => {
            let current = scene(state, content);
            let valid = current.choices.get(*idx).map(|c| c.enabled).unwrap_or(false);
            if valid {
                engine::choose(&mut next, *idx, content, &mut events);
            }
        }
        Command::CatchUp { days } => engine::catch_up(&mut next, *days, &mut events),
    }
    (next, events)
}

pub(crate) fn storylet<'c>(content: &'c ContentDb, id: &str) -> Option<&'c Storylet> {
    content.storylets.iter().find(|s| s.id == id)
}

/// Stable hash of the full world state, for golden-replay tests.
pub fn world_hash(state: &WorldState) -> u64 {
    let json = serde_json::to_string(state).expect("WorldState is always serializable");
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

pub(crate) fn narrate(w: &WorldState, events: &mut Vec<Event>, text: impl Into<String>) {
    events.push(Event::Narration { tick: w.tick, text: text.into() });
}

/// d12 + skill, the only dice in the game.
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

    /// Drive one complete Run with a fixed policy: accept, fuel up, depart,
    /// then always take the first enabled choice until back in Town.
    fn scripted_run(w: WorldState, c: &ContentDb) -> WorldState {
        let mut w = w;
        for idx in [2usize, 1, 3] {
            let (next, _) = step(&w, &Command::Choose { idx }, c);
            w = next;
        }
        let mut guard = 0;
        while !matches!(w.run.stage, RunStage::Town) {
            let s = scene(&w, c);
            let idx = s
                .choices
                .iter()
                .position(|ch| ch.enabled)
                .expect("every scene must have an enabled choice");
            let (next, _) = step(&w, &Command::Choose { idx }, c);
            w = next;
            guard += 1;
            assert!(guard < 200, "run did not terminate");
        }
        w
    }

    #[test]
    fn content_loads_and_validates() {
        let c = content();
        assert!(c.storylets.len() >= 8, "deck too thin: {}", c.storylets.len());
        assert!(c.settlements.len() >= 4);
        assert!(c.npcs.len() >= 5);
    }

    #[test]
    fn golden_replay_full_run_is_bit_exact() {
        let c = content();
        let a = scripted_run(WorldState::new(0xC0BB, &c), &c);
        let b = scripted_run(WorldState::new(0xC0BB, &c), &c);
        assert_eq!(world_hash(&a), world_hash(&b));
    }

    #[test]
    fn different_seeds_diverge() {
        let c = content();
        let a = scripted_run(WorldState::new(1, &c), &c);
        let b = scripted_run(WorldState::new(2, &c), &c);
        assert_ne!(world_hash(&a), world_hash(&b));
    }

    #[test]
    fn a_run_completes_and_pays() {
        let c = content();
        let w = scripted_run(WorldState::new(0xC0BB, &c), &c);
        assert_eq!(w.runs_completed, 1);
        assert!(w.cash_pence > 0);
        assert!(!w.run.accepted, "new day offers a fresh contract");
        assert_eq!(w.tick % PHASES_PER_DAY, 0, "ledger close lands on a dawn");
    }

    #[test]
    fn runs_vary_by_seed_and_dest() {
        let c = content();
        let mut beat_sets = BTreeSet::new();
        for seed in 0..12u64 {
            let mut w = WorldState::new(seed, &c);
            for idx in [2usize, 1, 3] {
                let (next, _) = step(&w, &Command::Choose { idx }, &c);
                w = next;
            }
            beat_sets.insert(w.run.beats.clone());
        }
        assert!(beat_sets.len() >= 4, "deck draws too samey: {beat_sets:?}");
    }

    #[test]
    fn save_load_roundtrip_preserves_replay() {
        let c = content();
        let mut w = WorldState::new(0xC0BB, &c);
        for idx in [2usize, 1, 3, 0] {
            let (next, _) = step(&w, &Command::Choose { idx }, &c);
            w = next;
        }
        let json = serde_json::to_string(&w).unwrap();
        let restored: WorldState = serde_json::from_str(&json).unwrap();
        let (a, _) = step(&w, &Command::Choose { idx: 0 }, &c);
        let (b, _) = step(&restored, &Command::Choose { idx: 0 }, &c);
        assert_eq!(world_hash(&a), world_hash(&b));
    }

    #[test]
    fn invariants_hold_over_many_runs_and_catchups() {
        let c = content();
        let mut w = WorldState::new(42, &c);
        for _ in 0..25 {
            w = scripted_run(w, &c);
            let (next, _) = step(&w, &Command::CatchUp { days: 3 }, &c);
            w = next;
        }
        assert!((8..=48).contains(&w.diesel_price_pence));
        assert!(w.cash_pence >= 0, "cash went negative: {}", w.cash_pence);
        assert!((0..=10).contains(&w.bedford_wear));
        assert!((0..=10).contains(&w.guild_suspicion));
        for v in w.factions.values() {
            assert!((-5..=5).contains(v));
        }
        assert_eq!(w.runs_completed, 25);
    }

    #[test]
    fn disabled_and_out_of_range_choices_are_noops() {
        let c = content();
        let w = WorldState::new(7, &c);
        let (after, ev) = step(&w, &Command::Choose { idx: 3 }, &c); // depart before accepting
        assert_eq!(world_hash(&w), world_hash(&after));
        assert!(ev.is_empty());
        let (after, _) = step(&w, &Command::Choose { idx: 99 }, &c);
        assert_eq!(world_hash(&w), world_hash(&after));
    }
}
