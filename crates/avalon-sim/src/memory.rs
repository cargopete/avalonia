//! NPC memory canon (Stage 4). Memories are part of the world state — the
//! sim creates observations and propagates gossip deterministically at Ledger
//! Close; the LLM may later *rewrite the wording* of an entry (recorded-oracle
//! command), but who-knows-what is decided here, purely.

use crate::*;

pub const MEMORIES_PER_NPC: usize = 30;

/// Canonical attributes of a fact — the Inquiry's ground truth lives in
/// `WorldState.facts`; memories carry their own (possibly distorted) copy.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FactAttrs {
    pub actor: String,
    pub action: String,
    pub location: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FactRec {
    pub fact_id: String,
    pub tick: Tick,
    pub attrs: FactAttrs,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MemoryEntry {
    pub id: u64,
    pub npc: String,
    /// "observation" | "gossip" | "reflection" | "chat"
    pub kind: String,
    pub text: String,
    /// Links to a FactRec when the memory is about a concrete happening.
    pub fact_id: Option<String>,
    /// The witness's own version of events. Gossip may distort this.
    pub asserts: Option<FactAttrs>,
    /// 1..=10, mundane to poignant (Park et al. 2023).
    pub importance: i64,
    pub tick: Tick,
}

/// flag -> (first-hand witness, importance, gossip recipient, charge prose).
/// The witness map is the social physics of the four roads.
pub fn flag_witness(flag: &str) -> Option<(&'static str, i64, Option<&'static str>)> {
    Some(match flag {
        "gate_smashed" => ("hobbs", 8, Some("tansy")),
        "bribed_checkpoint" => ("hobbs", 6, Some("carver")),
        "helped_traveller" => ("wray", 6, Some("tansy")),
        "passed_traveller" => ("wray", 4, None),
        "paid_carver" => ("carver", 5, None),
        "refused_carver" => ("carver", 6, Some("tansy")),
        "took_salvage" => ("tansy", 5, Some("finch")),
        "ran_ambush" => ("carver", 5, None),
        "yielded_to_pulver" => ("tansy", 3, None),
        "shaken_down" => ("tansy", 3, None),
        "took_drove_lane" => ("finch", 3, None),
        "owes_form_4b" => ("finch", 4, Some("arthur")),
        _ => return None,
    })
}

/// Flags grave enough to convene an Inquiry over.
pub fn chargeable(flag: &str) -> Option<&'static str> {
    Some(match flag {
        "gate_smashed" => "wilful destruction of Collective property, to wit one pole",
        "bribed_checkpoint" => "corruption of a barrier officer in the execution of his kettle",
        "took_salvage" => "appropriation of goods in transit, forty tins thereof",
        "owes_form_4b" => "failure to apply for Form 4B within the prescribed Tuesday",
        _ => return None,
    })
}

fn memory_text(flag: &str, attrs: &FactAttrs, gossip: bool) -> String {
    let deed = match flag {
        "gate_smashed" => "took the barrier pole off its bracket at speed",
        "bribed_checkpoint" => "paid the 'paperwork fee' at the barrier",
        "helped_traveller" => "stopped for a stranded rider and got him running",
        "passed_traveller" => "drove past a man broken down on the verge",
        "paid_carver" => "paid the road maintenance toll without fuss",
        "refused_carver" => "refused the toll and cited the highways acts",
        "took_salvage" => "loaded tins from an overturned cart",
        "ran_ambush" => "drove straight through a staged breakdown",
        "yielded_to_pulver" => "gave way to Pulver's Leyland on the straight",
        "shaken_down" => "paid a 'toll' to men with clean coats",
        "took_drove_lane" => "took the drove lane to dodge the barrier",
        "owes_form_4b" => "was found carrying a 4C where a 4B was wanted",
        _ => "did something on the road",
    };
    let actor = if attrs.actor == "cobb" { "Cobb" } else { &attrs.actor };
    if gossip {
        format!("They say {actor} {deed}, out by {}.", attrs.location)
    } else {
        format!("{actor} {deed}, on the {} road.", attrs.location)
    }
}

pub(crate) fn add_memory(w: &mut WorldState, mut entry: MemoryEntry) {
    entry.id = w.next_memory_id;
    w.next_memory_id += 1;
    w.memories.push(entry);
    // Cap per NPC: drop the most forgettable (lowest importance, then oldest).
    let npc = w.memories.last().unwrap().npc.clone();
    let count = w.memories.iter().filter(|m| m.npc == npc).count();
    if count > MEMORIES_PER_NPC {
        if let Some(pos) = w
            .memories
            .iter()
            .enumerate()
            .filter(|(_, m)| m.npc == npc)
            .min_by_key(|(_, m)| (m.importance, std::cmp::Reverse(m.tick)))
            .map(|(i, _)| i)
        {
            w.memories.remove(pos);
        }
    }
}

/// Called at Ledger Close with the run's flags: writes canonical facts,
/// first-hand observations, and (deterministically distorted) gossip.
pub(crate) fn record_run_memories(w: &mut WorldState, run_flags: &BTreeSet<String>, content: &ContentDb) {
    // Arthur clocks every illicit delivery, mildly.
    if w.run.contract.illicit {
        let attrs = FactAttrs {
            actor: "cobb".into(),
            action: "ran_cargo".into(),
            location: w.run.contract.dest_id.clone(),
        };
        add_memory(
            w,
            MemoryEntry {
                id: 0,
                npc: "arthur".into(),
                kind: "observation".into(),
                text: format!(
                    "Cobb ran {} to {} and the docket never existed.",
                    w.run.contract.cargo, w.run.contract.dest_name
                ),
                fact_id: None,
                asserts: Some(attrs),
                importance: 3,
                tick: w.tick,
            },
        );
    }

    for flag in run_flags {
        let Some((witness, importance, gossip_to)) = flag_witness(flag) else {
            continue;
        };
        let fact_id = format!("run{}-{}", w.runs_completed.saturating_sub(1), flag);
        let attrs = FactAttrs {
            actor: "cobb".into(),
            action: flag.clone(),
            location: w.run.contract.dest_id.clone(),
        };
        w.facts.push(FactRec { fact_id: fact_id.clone(), tick: w.tick, attrs: attrs.clone() });

        add_memory(
            w,
            MemoryEntry {
                id: 0,
                npc: witness.into(),
                kind: "observation".into(),
                text: memory_text(flag, &attrs, false),
                fact_id: Some(fact_id.clone()),
                asserts: Some(attrs.clone()),
                importance,
                tick: w.tick,
            },
        );

        if let Some(to) = gossip_to {
            // Gossip distorts one attribute, deterministically: usually the
            // place, sometimes the actor. This is what the Inquiry catches.
            let mut distorted = attrs.clone();
            match w.rng.gen_range(0..3) {
                0 => {
                    let others: Vec<_> = content
                        .settlements
                        .iter()
                        .filter(|s| s.id != distorted.location)
                        .collect();
                    if !others.is_empty() {
                        let pick = w.rng.gen_range(0..others.len());
                        distorted.location = others[pick].id.clone();
                    }
                }
                1 => distorted.actor = "one of Pulver's drivers".into(),
                _ => {} // faithful retelling — it happens
            }
            let text = memory_text(flag, &distorted, true);
            add_memory(
                w,
                MemoryEntry {
                    id: 0,
                    npc: to.into(),
                    kind: "gossip".into(),
                    text,
                    fact_id: Some(fact_id),
                    asserts: Some(distorted),
                    importance: (importance - 2).max(1),
                    tick: w.tick,
                },
            );
        }
    }
}
