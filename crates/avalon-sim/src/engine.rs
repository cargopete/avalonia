//! The mission engine: generation, gates, lazy beat draw, skill tests,
//! effects, and the win/loss flow. All prose comes from the ContentDb or the
//! framing templates here. Pure: operates on Mission, no IO.

use crate::*;
use avalon_content::{Effects, Gates, SChoice, Storylet, Terrain, Test};

const START_CASH_PENCE: i64 = 4_320; // £18 of spending money for fuel and bribes

// ----------------------------------------------------------- generation

pub(crate) fn generate(seed: u64, content: &ContentDb) -> Mission {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);

    let terrain: &Terrain = &content.terrains[rng.gen_range(0..content.terrains.len())];
    let cargo = &content.cargoes[rng.gen_range(0..content.cargoes.len())];
    let dest_name = if content.settlements.is_empty() {
        "the border".to_string()
    } else {
        content.settlements[rng.gen_range(0..content.settlements.len())].name.clone()
    };
    let diesel_price_pence = rng.gen_range(14..=22);
    let reward_pence = cargo.base_pay_pence
        + terrain.legs as i64 * 240
        + if cargo.illicit { 360 } else { 0 }
        + rng.gen_range(0..=8) * 25;

    let fixer = npc_or(content, "arthur");
    let antagonist = pick_present(
        &mut rng,
        content,
        &["hobbs", "finch", "carver"],
    );

    let mut m = Mission {
        seed,
        terrain: terrain.id.clone(),
        terrain_name: terrain.name.clone(),
        terrain_flavor: terrain.flavor.clone(),
        cargo: cargo.name.clone(),
        qty_desc: cargo.qty_desc.clone(),
        dest_name,
        illicit: cargo.illicit,
        reward_pence,
        fixer,
        antagonist,
        cash_pence: START_CASH_PENCE,
        diesel_price_pence,
        fuel_ml: terrain.start_fuel_l * 1_000,
        max_fuel_ml: terrain.max_fuel_l * 1_000,
        leg_fuel_ml: terrain.leg_fuel_l * 1_000,
        wear: terrain.start_wear.clamp(0, MAX_WEAR),
        heat: 0,
        dc_mod: terrain.dc_mod,
        heat_per_leg: terrain.heat_per_leg,
        wear_per_leg: terrain.wear_per_leg,
        legs_total: terrain.legs as usize,
        leg: 0,
        beats: Vec::new(),
        stage: Stage::Briefing,
        outcome: Outcome::InProgress,
        flags: BTreeSet::new(),
        talked: false,
        cargo_lost: false,
        rng,
    };
    // Keep the truck's RNG out of generation's stream by re-seeding play from
    // a derived seed, so editing generation later doesn't reshuffle the road.
    m.rng = ChaCha8Rng::seed_from_u64(seed ^ 0x5DEE_CE66);
    m
}

fn npc_or(content: &ContentDb, want: &str) -> String {
    if content.npcs.iter().any(|n| n.id == want) {
        want.to_string()
    } else {
        content.npcs.first().map(|n| n.id.clone()).unwrap_or_default()
    }
}

fn pick_present(rng: &mut ChaCha8Rng, content: &ContentDb, pool: &[&str]) -> String {
    let present: Vec<&str> =
        pool.iter().copied().filter(|id| content.npcs.iter().any(|n| n.id == *id)).collect();
    if present.is_empty() {
        String::new()
    } else {
        present[rng.gen_range(0..present.len())].to_string()
    }
}

// -------------------------------------------------------------------- gates

fn gates_pass(g: &Gates, m: &Mission) -> bool {
    if g.min_heat.is_some_and(|v| m.heat < v) {
        return false;
    }
    if g.max_heat.is_some_and(|v| m.heat > v) {
        return false;
    }
    if g.min_wear.is_some_and(|v| m.wear < v) {
        return false;
    }
    if g.max_wear.is_some_and(|v| m.wear > v) {
        return false;
    }
    if g.illicit.is_some_and(|v| m.illicit != v) {
        return false;
    }
    if let Some(f) = &g.flag {
        if !m.flags.contains(f) {
            return false;
        }
    }
    if let Some(f) = &g.not_flag {
        if m.flags.contains(f) {
            return false;
        }
    }
    if !g.terrains.is_empty() && !g.terrains.contains(&m.terrain) {
        return false;
    }
    true
}

fn template_of<'c>(content: &'c ContentDb, id: &str) -> Option<&'c str> {
    storylet(content, id).map(|s| s.template.as_str())
}

/// Lazily draw the next beat given current state: eligible, terrain-matched,
/// no repeated template this run, weighted. None when the pool is dry.
fn draw_next(m: &mut Mission, content: &ContentDb) -> Option<String> {
    let used: BTreeSet<&str> =
        m.beats.iter().filter_map(|id| template_of(content, id)).collect();
    let pool: Vec<&Storylet> = content
        .storylets
        .iter()
        .filter(|s| gates_pass(&s.requires, m) && !used.contains(s.template.as_str()))
        .collect();
    if pool.is_empty() {
        return None;
    }
    let total: u32 = pool.iter().map(|s| s.weight.max(1)).sum();
    let mut pick = m.rng.gen_range(0..total);
    for s in &pool {
        let wgt = s.weight.max(1);
        if pick < wgt {
            return Some(s.id.clone());
        }
        pick -= wgt;
    }
    Some(pool[0].id.clone())
}

// ----------------------------------------------------------- substitutions

fn subst(text: &str, m: &Mission) -> String {
    text.replace("{dest}", &m.dest_name)
        .replace("{cargo}", &m.cargo)
        .replace("{qty}", &m.qty_desc)
        .replace("{terrain}", &m.terrain_name)
        .replace("{reward}", &fmt_pence(m.reward_pence))
}

// ------------------------------------------------------------------- scenes

pub(crate) fn scene(m: &Mission, content: &ContentDb) -> Scene {
    match &m.stage {
        Stage::Briefing => briefing_scene(m),
        Stage::Beat { idx } => beat_scene(m, content, *idx),
        Stage::Outcome { text, .. } => Scene {
            title: format!("On {}", m.terrain_name),
            body: vec![text.clone()],
            choices: vec![Choice::on("Drive on")],
            speaker: None,
        },
        Stage::Arrival => arrival_scene(m),
        Stage::Over => over_scene(m),
    }
}

fn briefing_scene(m: &Mission) -> Scene {
    let mut body = vec![
        format!(
            "Wychford depot, before light. The job: {} of {} out across {}, to {}. \
             {} on delivery.",
            m.qty_desc,
            m.cargo,
            m.terrain_name,
            m.dest_name,
            fmt_pence(m.reward_pence)
        ),
        m.terrain_flavor.clone(),
    ];
    if m.illicit {
        body.push("No Form 4B exists for this load. That is rather the point.".into());
    }

    let diesel_cost = DIESEL_BUY_L * m.diesel_price_pence;
    let full = m.fuel_ml >= m.max_fuel_ml;
    let choices = vec![
        if m.talked {
            Choice::off("Press the fixer further", "He's told you what he'll tell you.")
        } else {
            Choice::on("Ask the fixer about the road")
        },
        if full {
            Choice::off("Top up the tank", "She's brimmed.")
        } else if m.cash_pence >= diesel_cost {
            Choice::on(format!(
                "Top up {DIESEL_BUY_L} L ({}) — tank {} L",
                fmt_pence(diesel_cost),
                m.fuel_l()
            ))
        } else {
            Choice::off(
                format!("Top up {DIESEL_BUY_L} L ({})", fmt_pence(diesel_cost)),
                "You can't cover it.",
            )
        },
        Choice::on(format!("Set off for {}", m.dest_name)),
    ];
    Scene {
        title: "The Job".into(),
        body,
        choices,
        speaker: Some(m.fixer.clone()),
    }
}

fn beat_scene(m: &Mission, content: &ContentDb, idx: usize) -> Scene {
    let Some(st) = m.beats.get(idx).and_then(|id| storylet(content, id)) else {
        return Scene {
            title: format!("On {}", m.terrain_name),
            body: vec!["The road runs on, empty and without incident, which out here \
                        counts as a mercy."
                .into()],
            choices: vec![Choice::on("Drive on")],
            speaker: None,
        };
    };
    let choices = st
        .choices
        .iter()
        .map(|c| match c.requires_cash {
            Some(need) if m.cash_pence < need => {
                Choice::off(subst(&c.label, m), "You can't cover it.")
            }
            _ => Choice::on(subst(&c.label, m)),
        })
        .collect();
    Scene {
        title: subst(&st.title, m),
        body: st.body.iter().map(|p| subst(p, m)).collect(),
        choices,
        speaker: template_speaker(&st.template, m),
    }
}

/// Which face fronts a beat, for the portrait.
fn template_speaker(template: &str, m: &Mission) -> Option<String> {
    match template {
        "checkpoint" => Some(if m.heat >= 4 { "finch".into() } else { "hobbs".into() }),
        "shakedown" => Some("carver".into()),
        "traveller" => Some("wray".into()),
        _ => None,
    }
}

fn arrival_scene(m: &Mission) -> Scene {
    Scene {
        title: format!("{} — the drop", m.dest_name),
        body: vec![format!(
            "The yard at {} is lamp-lit and businesslike. {}",
            m.dest_name,
            if m.illicit {
                "Nobody asks what's under the sheeting, which is its own kind of manners."
            } else {
                "The manifest is read aloud, slowly, as if it were scripture."
            }
        )],
        choices: vec![Choice::on("Hand over the cargo")],
        speaker: None,
    }
}

fn over_scene(m: &Mission) -> Scene {
    let (title, lines) = match &m.outcome {
        Outcome::Won { lines, .. } => ("Delivered".to_string(), lines.clone()),
        Outcome::Lost { reason, lines } => (format!("Lost — {reason}"), lines.clone()),
        Outcome::InProgress => ("—".to_string(), vec![]),
    };
    Scene { title, body: lines, choices: vec![Choice::on("Begin a new run")], speaker: None }
}

// -------------------------------------------------------------- transitions

pub(crate) fn choose(m: &mut Mission, idx: usize, content: &ContentDb, events: &mut Vec<Event>) {
    match m.stage.clone() {
        Stage::Briefing => briefing_choose(m, idx, content, events),
        Stage::Beat { idx: beat } => {
            let text = resolve_beat(m, content, beat, idx, events);
            narrate(events, text.clone());
            // A beat's effects may have ended the run (caught, broke down,
            // cargo seized). Otherwise show the outcome and drive on.
            if let Some((reason, lines)) = check_failure(m) {
                lose(m, reason, lines, events);
            } else {
                m.stage = Stage::Outcome { idx: beat, text };
            }
        }
        Stage::Outcome { idx: beat, .. } => {
            if beat + 1 < m.legs_total {
                advance_to_beat(m, content, beat + 1, events);
            } else {
                m.stage = Stage::Arrival;
            }
        }
        Stage::Arrival => deliver(m, events),
        Stage::Over => {} // new run is a daemon concern (fresh seed)
    }
}

fn briefing_choose(m: &mut Mission, idx: usize, content: &ContentDb, events: &mut Vec<Event>) {
    match idx {
        0 => {
            m.talked = true;
            narrate(events, briefing_line(m));
        }
        1 => {
            let cost = DIESEL_BUY_L * m.diesel_price_pence;
            if m.cash_pence >= cost && m.fuel_ml < m.max_fuel_ml {
                m.cash_pence -= cost;
                m.fuel_ml = (m.fuel_ml + DIESEL_BUY_L * 1_000).min(m.max_fuel_ml);
                narrate(
                    events,
                    format!(
                        "{DIESEL_BUY_L} litres in, {} the litre. The pump counts it out \
                         like a creditor.",
                        fmt_pence(m.diesel_price_pence)
                    ),
                );
            }
        }
        2 => {
            narrate(
                events,
                format!(
                    "The Bedford pulls out of Wychford loaded with {} of {}, nose set for \
                     {}. The town does not wave.",
                    m.qty_desc, m.cargo, m.dest_name
                ),
            );
            advance_to_beat(m, content, 0, events);
        }
        _ => {}
    }
}

fn briefing_line(m: &Mission) -> String {
    let road = match m.terrain.as_str() {
        "moor" => "Stay off the skyline where you can. The spotters have nothing to do but watch.",
        "fen" => "Mind the causeways. The water's been at them and the potholes bite axles.",
        "forest" => "Pulver's lot work the bends. If a Leyland sits on your tail, it means it.",
        "coast" => "There's a cutter off the headland counting lights. Don't give them an even number.",
        "pass" => "Nurse her up the grades. A boiled engine on the pass is a long cold wait.",
        _ => "Watch yourself out there.",
    };
    let who = if m.antagonist.is_empty() {
        "Nobody's named, which means everybody."
    } else {
        "Somebody's been asking after the truck. You know how that goes."
    };
    format!("The fixer, not looking up: \"{road} {who}\"")
}

/// Drive one leg onto beat `idx`: burn fuel, take terrain's toll, draw the
/// beat, then check whether the leg alone has ended the run.
fn advance_to_beat(m: &mut Mission, content: &ContentDb, idx: usize, events: &mut Vec<Event>) {
    m.leg = idx + 1;
    m.fuel_ml -= m.leg_fuel_ml;
    if m.fuel_ml <= 0 {
        if m.cash_pence >= JERRY_CAN_PENCE {
            m.cash_pence -= JERRY_CAN_PENCE;
            m.fuel_ml += JERRY_CAN_ML;
            narrate(
                events,
                format!(
                    "The tank runs dry on the open road. A farm sells you a can at \
                     robbery rates ({}).",
                    fmt_pence(JERRY_CAN_PENCE)
                ),
            );
        } else {
            m.fuel_ml = 0;
            lose(
                m,
                "stranded".into(),
                vec![
                    "The needle sits on the pin and the road sits empty. No fuel, no \
                     fund, no farm in sight."
                        .into(),
                    format!("{} of {} go nowhere tonight.", m.qty_desc, m.cargo),
                ],
                events,
            );
            return;
        }
    }
    m.heat = (m.heat + m.heat_per_leg).clamp(0, MAX_HEAT);
    m.wear = (m.wear + m.wear_per_leg).clamp(0, MAX_WEAR);

    if let Some((reason, lines)) = check_failure(m) {
        lose(m, reason, lines, events);
        return;
    }
    match draw_next(m, content) {
        Some(id) => {
            if m.beats.len() <= idx {
                m.beats.resize(idx + 1, String::new());
            }
            m.beats[idx] = id;
            m.stage = Stage::Beat { idx };
        }
        None => m.stage = Stage::Arrival,
    }
}

fn apply_effects(m: &mut Mission, e: &Effects) {
    if let Some(v) = e.cash {
        m.cash_pence = (m.cash_pence + v).max(0);
    }
    if let Some(v) = e.fuel_ml {
        m.fuel_ml = (m.fuel_ml + v).clamp(0, m.max_fuel_ml);
    }
    if let Some(v) = e.wear {
        m.wear = (m.wear + v).clamp(0, MAX_WEAR);
    }
    if let Some(v) = e.heat {
        m.heat = (m.heat + v).clamp(0, MAX_HEAT);
    }
    if let Some(v) = e.payment_pct {
        m.reward_pence = (m.reward_pence * (100 + v).max(0) / 100).max(0);
    }
    if e.lose_cargo.unwrap_or(false) {
        m.cargo_lost = true;
    }
    if let Some(f) = &e.flag {
        m.flags.insert(f.clone());
    }
}

fn resolve_test(m: &mut Mission, t: &Test) -> bool {
    let mut dc = t.dc + m.dc_mod;
    if let Some(div) = t.dc_wear_div {
        if div > 0 {
            dc += m.wear / div;
        }
    }
    if let Some(bump) = t.dc_heat_bump {
        if m.heat >= 4 {
            dc += bump;
        }
    }
    roll(&mut m.rng, skill_value(&t.skill)) >= dc
}

fn resolve_beat(
    m: &mut Mission,
    content: &ContentDb,
    beat: usize,
    choice: usize,
    _events: &mut Vec<Event>,
) -> String {
    let Some(st) = m.beats.get(beat).and_then(|id| storylet(content, id)) else {
        return "The road continues, indifferent.".into();
    };
    let Some(ch): Option<&SChoice> = st.choices.get(choice) else {
        return "The road continues, indifferent.".into();
    };
    let ch = ch.clone();
    match &ch.test {
        Some(t) if !resolve_test(m, t) => {
            if let Some(fe) = &ch.fail_effects {
                apply_effects(m, fe);
            }
            subst(ch.fail_outcome.as_deref().unwrap_or("It does not go well."), m)
        }
        _ => {
            apply_effects(m, &ch.effects);
            subst(&ch.outcome, m)
        }
    }
}

/// The earned loss conditions. Order matters only for the reported reason.
fn check_failure(m: &Mission) -> Option<(String, Vec<String>)> {
    if m.heat >= MAX_HEAT {
        return Some((
            "caught".into(),
            vec![
                "The block comes out of nowhere and everywhere at once — they have had \
                 long enough to arrange it. Hands on the bonnet. The sheeting comes off."
                    .into(),
                format!("{} of {} change owner, and not to your profit.", m.qty_desc, m.cargo),
            ],
        ));
    }
    if m.wear >= MAX_WEAR {
        return Some((
            "broken down".into(),
            vec![
                "Something vital lets go with a noise like a dropped anvil, and the \
                 Bedford coasts to a stop that has the air of being permanent."
                    .into(),
                "By the time help comes, so has everyone else.".into(),
            ],
        ));
    }
    if m.cargo_lost {
        return Some((
            "cargo gone".into(),
            vec!["Whatever was under the sheeting is under it no longer. A delivery of \
                  fresh air pays the same as the air."
                .into()],
        ));
    }
    None
}

fn lose(m: &mut Mission, reason: String, lines: Vec<String>, events: &mut Vec<Event>) {
    narrate(events, lines.first().cloned().unwrap_or_default());
    m.outcome = Outcome::Lost { reason, lines };
    m.stage = Stage::Over;
}

fn deliver(m: &mut Mission, events: &mut Vec<Event>) {
    if let Some((reason, lines)) = check_failure(m) {
        lose(m, reason, lines, events);
        return;
    }
    let haul = m.reward_pence;
    narrate(events, format!("Counted twice, paid once: {}.", fmt_pence(haul)));
    let mut lines = vec![
        format!("{} of {} delivered to {}.", m.qty_desc, m.cargo, m.dest_name),
        format!("Haul: {}. Pocket on the road: {}.", fmt_pence(haul), fmt_pence(m.cash_pence)),
        format!("Brought her in at wear {}/10, heat {}/10.", m.wear, m.heat),
    ];
    for flag in &m.flags {
        if let Some(line) = epitaph(flag) {
            lines.push(line.into());
        }
    }
    m.outcome = Outcome::Won { haul_pence: haul, lines };
    m.stage = Stage::Over;
}

fn epitaph(flag: &str) -> Option<&'static str> {
    Some(match flag {
        "gate_smashed" => "You left a Collective pole in splinters. They'll remember the plate.",
        "bribed_checkpoint" => "A cash box on the road knows your face now.",
        "helped_traveller" => "A seed merchant owes you a kindness. It may even be collected.",
        "took_salvage" => "Forty unprovenanced tins rode home under the sheeting.",
        "ran_ambush" => "Somebody planned an evening around stopping you, and didn't.",
        "refused_carver" => "You told Carver's men no, to their faces. Word gets about.",
        "paid_carver" => "Carver's notebook has a tick by your name.",
        _ => return None,
    })
}
