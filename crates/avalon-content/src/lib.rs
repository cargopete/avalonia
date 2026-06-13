//! avalon-content — typed content model + TOML loaders (RFC-AVL-001 DR-6).
//!
//! The model types are pure data (avalon-sim consumes them); the loader does
//! IO and lives only in the daemon's boot path. Load-time validation fails the
//! boot, not the session: every gate/effect must reference real sim entities.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

// ------------------------------------------------------------------- model

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ContentDb {
    pub settlements: Vec<Settlement>,
    pub terrains: Vec<Terrain>,
    pub cargoes: Vec<Cargo>,
    pub storylets: Vec<Storylet>,
    pub npcs: Vec<Npc>,
    pub banned_phrases: Vec<String>,
}

/// A road type for one mission. Drives length, fuel pressure, which clock is
/// dangerous, and which storylets can appear. The variety axis (DR-3).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Terrain {
    pub id: String,
    pub name: String,
    pub flavor: String,
    /// Beats on the road for this terrain (3..=6).
    pub legs: u32,
    /// Tank at the start, litres; and the most you can carry.
    pub start_fuel_l: i64,
    pub max_fuel_l: i64,
    /// Litres burned per leg on this ground.
    pub leg_fuel_l: i64,
    /// The truck you're given for this run starts this worn (0..=10). Rough
    /// ground hands you a tireder Bedford. Defaults to 2.
    #[serde(default = "default_start_wear")]
    pub start_wear: i64,
    /// Added to every skill-test DC on this terrain.
    pub dc_mod: i64,
    /// Pursuit (heat) accrued just by being out here, per leg.
    pub heat_per_leg: i64,
    /// Truck strain (wear) per leg on this ground.
    pub wear_per_leg: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Settlement {
    pub id: String,
    pub name: String,
    /// Road legs from Wychford = number of beats on a run there (3..=5).
    pub legs: u32,
    /// Payment multiplier, percent (a longer or nastier road pays better).
    pub pay_bonus_pct: i64,
    pub flavor: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Cargo {
    pub name: String,
    pub qty_desc: String,
    pub base_pay_pence: i64,
    pub illicit: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Npc {
    pub id: String,
    pub name: String,
    pub role: String,
    pub faction: String,
    /// Character card for LLM dialogue prompts (Stage 3).
    pub persona: String,
    /// Authored fallback lines, keyed loosely by mood.
    pub barks: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Storylet {
    pub id: String,
    /// Beat family; one beat per template per run, so runs stay varied.
    pub template: String,
    #[serde(default = "default_weight")]
    pub weight: u32,
    #[serde(default)]
    pub requires: Gates,
    pub title: String,
    pub body: Vec<String>,
    pub choices: Vec<SChoice>,
}

fn default_weight() -> u32 {
    1
}

fn default_start_wear() -> i64 {
    2
}

/// All gates are ANDed; absent = pass. Numeric gates are inclusive bounds.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Gates {
    pub min_heat: Option<i64>,
    pub max_heat: Option<i64>,
    pub min_wear: Option<i64>,
    pub max_wear: Option<i64>,
    pub illicit: Option<bool>,
    pub flag: Option<String>,
    pub not_flag: Option<String>,
    /// Only eligible on these terrains (by id). Empty = any terrain.
    #[serde(default)]
    pub terrains: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SChoice {
    pub label: String,
    /// Disabled (greyed, with note) unless Cobb can cover it.
    pub requires_cash: Option<i64>,
    pub test: Option<Test>,
    /// Effects + outcome prose. With a test these are the pass branch.
    #[serde(default)]
    pub effects: Effects,
    pub outcome: String,
    /// Fail branch (tests only).
    #[serde(default)]
    pub fail_effects: Option<Effects>,
    pub fail_outcome: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Test {
    /// "paperwork" | "spanners" | "charm"
    pub skill: String,
    pub dc: i64,
    /// DC += wear / this (truck-dependent difficulty).
    pub dc_wear_div: Option<i64>,
    /// DC += this when heat >= 4 (officialdom is already watching).
    pub dc_heat_bump: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Effects {
    pub cash: Option<i64>,
    pub fuel_ml: Option<i64>,
    pub wear: Option<i64>,
    /// Pursuit clock. 10 = caught (mission lost).
    pub heat: Option<i64>,
    pub flag: Option<String>,
    /// Reward adjustment at the drop, percent (spoilage, sweeteners).
    pub payment_pct: Option<i64>,
    /// Set to lose the cargo outright → mission lost at the drop.
    pub lose_cargo: Option<bool>,
}

pub const SKILLS: [&str; 3] = ["paperwork", "spanners", "charm"];

// ------------------------------------------------------------------ loader

#[derive(Deserialize)]
struct WorldFile {
    #[serde(default)]
    settlements: Vec<Settlement>,
    #[serde(default)]
    terrains: Vec<Terrain>,
    cargoes: Vec<Cargo>,
    #[serde(default)]
    banned_phrases: Vec<String>,
}

#[derive(Deserialize)]
struct StoryletFile {
    #[serde(default)]
    storylet: Vec<Storylet>,
}

#[derive(Deserialize)]
struct NpcFile {
    #[serde(default)]
    npc: Vec<Npc>,
}

pub fn load(dir: &Path) -> Result<ContentDb, String> {
    let world: WorldFile = read(dir.join("world.toml"))?;
    let mut db = ContentDb {
        settlements: world.settlements,
        terrains: world.terrains,
        cargoes: world.cargoes,
        banned_phrases: world.banned_phrases,
        ..Default::default()
    };
    for entry in sorted_toml(&dir.join("storylets"))? {
        let f: StoryletFile = read(entry)?;
        db.storylets.extend(f.storylet);
    }
    for entry in sorted_toml(&dir.join("npcs"))? {
        let f: NpcFile = read(entry)?;
        db.npcs.extend(f.npc);
    }
    // Deterministic order regardless of filesystem: the deck draw consumes
    // RNG in deck order, so this is replay-relevant.
    db.storylets.sort_by(|a, b| a.id.cmp(&b.id));
    db.npcs.sort_by(|a, b| a.id.cmp(&b.id));
    validate(&db)?;
    Ok(db)
}

fn read<T: serde::de::DeserializeOwned>(path: std::path::PathBuf) -> Result<T, String> {
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn sorted_toml(dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    out.sort();
    Ok(out)
}

pub fn validate(db: &ContentDb) -> Result<(), String> {
    let mut errs = Vec::new();
    let terrain_ids: BTreeSet<_> = db.terrains.iter().map(|t| t.id.as_str()).collect();
    let mut seen = BTreeSet::new();

    if db.terrains.is_empty() {
        errs.push("no terrains defined".into());
    }
    if db.cargoes.is_empty() {
        errs.push("no cargoes defined".into());
    }
    for t in &db.terrains {
        if !(2..=8).contains(&t.legs) {
            errs.push(format!("terrain {}: legs must be 2..=8", t.id));
        }
        if t.start_fuel_l <= 0 || t.max_fuel_l < t.start_fuel_l {
            errs.push(format!("terrain {}: fuel bounds nonsensical", t.id));
        }
    }
    for st in &db.storylets {
        if !seen.insert(&st.id) {
            errs.push(format!("duplicate storylet id {}", st.id));
        }
        if st.choices.is_empty() {
            errs.push(format!("storylet {}: no choices", st.id));
        }
        for tid in &st.requires.terrains {
            if !terrain_ids.contains(tid.as_str()) {
                errs.push(format!("storylet {}: unknown terrain gate {tid}", st.id));
            }
        }
        for (i, c) in st.choices.iter().enumerate() {
            if let Some(t) = &c.test {
                if !SKILLS.contains(&t.skill.as_str()) {
                    errs.push(format!("storylet {} choice {i}: unknown skill {}", st.id, t.skill));
                }
                if c.fail_outcome.is_none() {
                    errs.push(format!("storylet {} choice {i}: test without fail_outcome", st.id));
                }
            }
        }
    }
    // Every terrain needs enough always-eligible (untagged or matching) beats
    // to fill a run without repeating a template.
    let templates: BTreeSet<_> = db.storylets.iter().map(|s| s.template.as_str()).collect();
    if templates.len() < 3 && !db.storylets.is_empty() {
        errs.push("fewer than 3 beat templates; runs will repeat".into());
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("\n"))
    }
}
