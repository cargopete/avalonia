//! The storylet engine: gates, deck draw, tests, effects, and the run
//! framing (town, arrival, ledger close). Operates on WorldState; all prose
//! comes from the ContentDb or the framing templates here.

use crate::*;
use avalon_content::{Cargo, Effects, Gates, SChoice, Settlement, Storylet, Test};

const FUEL_PER_LEG_ML: i64 = 3_000;
const DIESEL_BUY_L: i64 = 20;
const MIN_DEPART_FUEL_ML: i64 = 10_000;
const JERRY_CAN_PENCE: i64 = 240;

// ---------------------------------------------------------------- contracts

pub(crate) fn gen_contract(rng: &mut ChaCha8Rng, content: &ContentDb) -> Contract {
    let cargo: &Cargo = &content.cargoes[rng.gen_range(0..content.cargoes.len())];
    let dest: &Settlement = &content.settlements[rng.gen_range(0..content.settlements.len())];
    let base = cargo.base_pay_pence + rng.gen_range(0..=12) * 25;
    Contract {
        cargo: cargo.name.clone(),
        qty_desc: cargo.qty_desc.clone(),
        dest_id: dest.id.clone(),
        dest_name: dest.name.clone(),
        payment_pence: base * (100 + dest.pay_bonus_pct) / 100,
        illicit: cargo.illicit,
    }
}

fn rumour(rng: &mut ChaCha8Rng) -> &'static str {
    const RUMOURS: [&str; 5] = [
        "\"The Collective are stamping everything north of the river since somebody \
         took their gate off its bracket. Carry your papers where you can reach them.\"",
        "\"Carver's lot have been buying diesel by the drum. Either they're expecting \
         trouble, or they're planning to sell it back to us at double.\"",
        "\"Finch was in on Tuesday asking after your manifest. I told him the truth — \
         that I couldn't read your handwriting.\"",
        "\"That belt on the Bedford sounded like a kettle last time you pulled out. \
         See to it before the long climb.\"",
        "\"Pulver's boy has been running the night roads in that Leyland. Four fingers \
         at anyone he passes. One day somebody'll answer with five.\"",
    ];
    RUMOURS[rng.gen_range(0..RUMOURS.len())]
}

// -------------------------------------------------------------------- gates

fn gates_pass(g: &Gates, w: &WorldState) -> bool {
    let s = w.guild_suspicion;
    let wear = w.bedford_wear;
    if g.min_suspicion.is_some_and(|v| s < v) {
        return false;
    }
    if g.max_suspicion.is_some_and(|v| s > v) {
        return false;
    }
    if g.min_wear.is_some_and(|v| wear < v) {
        return false;
    }
    if g.max_wear.is_some_and(|v| wear > v) {
        return false;
    }
    if g.illicit.is_some_and(|v| w.run.contract.illicit != v) {
        return false;
    }
    if g.min_runs.is_some_and(|v| w.runs_completed < v) {
        return false;
    }
    if let Some(f) = &g.flag {
        if !w.flags.contains(f) && !w.run.flags.contains(f) {
            return false;
        }
    }
    if let Some(f) = &g.not_flag {
        if w.flags.contains(f) || w.run.flags.contains(f) {
            return false;
        }
    }
    if let Some(d) = &g.dest {
        if &w.run.contract.dest_id != d {
            return false;
        }
    }
    if let Some(fg) = &g.faction_rep_max {
        if w.faction(&fg.faction) > fg.value {
            return false;
        }
    }
    if let Some(fg) = &g.faction_rep_min {
        if w.faction(&fg.faction) < fg.value {
            return false;
        }
    }
    true
}

/// Draw the run's hand: `legs` beats, at most one per template, weighted.
fn draw_beats(w: &mut WorldState, content: &ContentDb, legs: u32) -> Vec<String> {
    let mut pool: Vec<&Storylet> =
        content.storylets.iter().filter(|s| gates_pass(&s.requires, w)).collect();
    let mut hand = Vec::new();
    for _ in 0..legs {
        if pool.is_empty() {
            break;
        }
        let total: u32 = pool.iter().map(|s| s.weight.max(1)).sum();
        let mut pick = w.rng.gen_range(0..total);
        let mut chosen = 0;
        for (i, s) in pool.iter().enumerate() {
            let wgt = s.weight.max(1);
            if pick < wgt {
                chosen = i;
                break;
            }
            pick -= wgt;
        }
        let template = pool[chosen].template.clone();
        hand.push(pool[chosen].id.clone());
        pool.retain(|s| s.template != template);
    }
    hand
}

// ----------------------------------------------------------- substitutions

fn subst(text: &str, w: &WorldState) -> String {
    let c = &w.run.contract;
    text.replace("{dest}", &c.dest_name)
        .replace("{cargo}", &c.cargo)
        .replace("{qty}", &c.qty_desc)
        .replace("{payment}", &fmt_pence(c.payment_pence))
}

// ------------------------------------------------------------------- scenes

pub(crate) fn scene(w: &WorldState, content: &ContentDb) -> Scene {
    match &w.run.stage {
        RunStage::Town => town_scene(w),
        RunStage::Beat { idx } => beat_scene(w, content, *idx),
        RunStage::Outcome { text, .. } => Scene {
            title: "On the Road".into(),
            body: vec![text.clone()],
            choices: vec![Choice::on("Drive on")],
        },
        RunStage::Arrival => arrival_scene(w),
        RunStage::Summary { lines } => Scene {
            title: "Ledger Close".into(),
            body: lines.clone(),
            choices: vec![Choice::on("Open the next day's ledger")],
        },
    }
}

fn town_scene(w: &WorldState) -> Scene {
    let c = &w.run.contract;
    let mut body = vec![
        "The Wychford depot smells of warm oil and wet sacking. The Bedford TK sits \
         under the awning, ticking as it cools, while Arthur Pidgeon works through a \
         stack of dockets with the air of a man besieged."
            .into(),
        format!(
            "Chalked on the board: {} of {}, Wychford to {}, {} on delivery. {}",
            c.qty_desc,
            c.cargo,
            c.dest_name,
            fmt_pence(c.payment_pence),
            if c.illicit {
                "Nobody has mentioned Form 4B. Nobody is going to."
            } else {
                "All perfectly above board, which is somehow worse."
            }
        ),
    ];
    if w.guild_suspicion >= 4 {
        body.push(
            "A notice by the door invites persons with knowledge of irregular haulage \
             to come forward. Somebody has drawn a moustache on it."
                .into(),
        );
    }

    let diesel_cost = DIESEL_BUY_L * w.diesel_price_pence;
    let choices = vec![
        if w.run.talked {
            Choice::off("Talk to Arthur", "He's said his piece for the day.")
        } else {
            Choice::on("Talk to Arthur")
        },
        if w.cash_pence >= diesel_cost {
            Choice::on(format!("Buy {DIESEL_BUY_L} L diesel ({})", fmt_pence(diesel_cost)))
        } else {
            Choice::off(
                format!("Buy {DIESEL_BUY_L} L diesel ({})", fmt_pence(diesel_cost)),
                "You can't cover it.",
            )
        },
        if w.run.accepted {
            Choice::off("Accept the contract", "Signed. After a fashion.")
        } else {
            Choice::on("Accept the contract")
        },
        if !w.run.accepted {
            Choice::off(
                format!("Depart for {}", c.dest_name),
                "No contract, no cargo, no point.",
            )
        } else if w.fuel_ml < MIN_DEPART_FUEL_ML {
            Choice::off(
                format!("Depart for {}", c.dest_name),
                format!("Tank's at {} L. You won't make the climb.", w.fuel_ml / 1_000),
            )
        } else {
            Choice::on(format!("Depart for {}", c.dest_name))
        },
    ];

    Scene { title: format!("Wychford Depot — {}", w.phase_name()), body, choices }
}

fn beat_scene(w: &WorldState, content: &ContentDb, idx: usize) -> Scene {
    let Some(st) = w.run.beats.get(idx).and_then(|id| storylet(content, id)) else {
        // A content edit removed a drawn beat mid-run: degrade to a quiet leg.
        return Scene {
            title: "The Open Road".into(),
            body: vec!["The road runs on, empty and wet and entirely without incident, \
                        which out here counts as a gift."
                .into()],
            choices: vec![Choice::on("Drive on")],
        };
    };
    let choices = st
        .choices
        .iter()
        .map(|c| match c.requires_cash {
            Some(need) if w.cash_pence < need => {
                Choice::off(subst(&c.label, w), "You can't cover it.")
            }
            _ => Choice::on(subst(&c.label, w)),
        })
        .collect();
    Scene {
        title: subst(&st.title, w),
        body: st.body.iter().map(|p| subst(p, w)).collect(),
        choices,
    }
}

fn arrival_scene(w: &WorldState) -> Scene {
    let c = &w.run.contract;
    Scene {
        title: format!("{} — The Yard", c.dest_name),
        body: vec![format!(
            "The yard at {} is lamp-lit and businesslike. A foreman with a pencil \
             behind each ear looks the Bedford over the way a farmer looks at weather. \
             {}",
            c.dest_name,
            if c.illicit {
                "Nobody asks what's under the sheeting, which is its own kind of manners."
            } else {
                "The manifest is read aloud, slowly, as if it were scripture."
            }
        )],
        choices: vec![Choice::on("Hand over the cargo")],
    }
}

// -------------------------------------------------------------- transitions

pub(crate) fn choose(w: &mut WorldState, idx: usize, content: &ContentDb, events: &mut Vec<Event>) {
    match w.run.stage.clone() {
        RunStage::Town => town_choose(w, idx, content, events),
        RunStage::Beat { idx: beat } => {
            let text = resolve_beat(w, content, beat, idx, events);
            narrate(w, events, text.clone());
            w.run.stage = RunStage::Outcome { idx: beat, text };
        }
        RunStage::Outcome { idx: beat, .. } => {
            enter_leg(w, events);
            if beat + 1 < w.run.beats.len() {
                w.run.stage = RunStage::Beat { idx: beat + 1 };
            } else {
                w.run.stage = RunStage::Arrival;
            }
        }
        RunStage::Arrival => deliver(w, content, events),
        RunStage::Summary { .. } => next_day(w, content, events),
    }
}

fn town_choose(w: &mut WorldState, idx: usize, content: &ContentDb, events: &mut Vec<Event>) {
    match idx {
        0 => {
            w.run.talked = true;
            let r = rumour(&mut w.rng);
            narrate(w, events, format!("Arthur, without looking up: {r}"));
        }
        1 => {
            let cost = DIESEL_BUY_L * w.diesel_price_pence;
            w.cash_pence -= cost;
            w.fuel_ml += DIESEL_BUY_L * 1_000;
            narrate(
                w,
                events,
                format!(
                    "{DIESEL_BUY_L} litres of diesel, {} the litre. The pump counts it \
                     out like a creditor.",
                    fmt_pence(w.diesel_price_pence)
                ),
            );
        }
        2 => {
            w.run.accepted = true;
            narrate(
                w,
                events,
                "You sign where signing is expected. Form 4B remains conspicuously blank.",
            );
        }
        3 => {
            w.run.start_cash = w.cash_pence;
            w.run.start_fuel_ml = w.fuel_ml;
            w.run.start_wear = w.bedford_wear;
            w.run.start_suspicion = w.guild_suspicion;
            let legs = content
                .settlements
                .iter()
                .find(|s| s.id == w.run.contract.dest_id)
                .map(|s| s.legs)
                .unwrap_or(3);
            w.run.beats = draw_beats(w, content, legs);
            narrate(
                w,
                events,
                format!(
                    "The Bedford pulls out of Wychford loaded with {} of {}. The town \
                     does not wave.",
                    w.run.contract.qty_desc, w.run.contract.cargo
                ),
            );
            enter_leg(w, events);
            w.run.stage = RunStage::Beat { idx: 0 };
        }
        _ => {}
    }
}

/// Every leg of road costs a phase and a few litres.
fn enter_leg(w: &mut WorldState, events: &mut Vec<Event>) {
    w.tick += 1;
    w.fuel_ml -= FUEL_PER_LEG_ML;
    if w.fuel_ml <= 0 {
        w.fuel_ml = 5_000;
        let cost = JERRY_CAN_PENCE.min(w.cash_pence);
        w.cash_pence -= cost;
        narrate(
            w,
            events,
            format!(
                "The tank runs dry short of anywhere useful. A farm sells you a jerry \
                 can at robbery rates ({}), plus the walk.",
                fmt_pence(cost)
            ),
        );
    }
}

fn apply_effects(w: &mut WorldState, e: &Effects, events: &mut Vec<Event>) {
    if let Some(v) = e.cash {
        w.cash_pence = (w.cash_pence + v).max(0);
    }
    if let Some(v) = e.fuel_ml {
        w.fuel_ml = (w.fuel_ml + v).max(0);
    }
    if let Some(v) = e.wear {
        w.bedford_wear = (w.bedford_wear + v).clamp(0, 10);
    }
    if let Some(v) = e.suspicion {
        w.guild_suspicion = (w.guild_suspicion + v).clamp(0, 10);
    }
    if let Some(v) = e.ticks {
        w.tick = w.tick.saturating_add_signed(v);
    }
    if let Some(v) = e.payment_pct {
        w.run.payment_pct += v;
    }
    if let Some(f) = &e.flag {
        w.run.flags.insert(f.clone());
        // Flags seeded on the road are facts: attributable, locatable, datable.
        events.push(Event::Fact {
            tick: w.tick,
            fact_id: format!("run{}-{}", w.runs_completed, f),
            actor: "cobb".into(),
            action: f.clone(),
            location: w.run.contract.dest_id.clone(),
        });
    }
    if let (Some(f), Some(d)) = (&e.faction, e.faction_delta) {
        let v = w.factions.entry(f.clone()).or_insert(0);
        *v = (*v + d).clamp(-5, 5);
    }
}

fn resolve_test(w: &mut WorldState, t: &Test) -> bool {
    let mut dc = t.dc;
    if let Some(div) = t.dc_wear_div {
        if div > 0 {
            dc += w.bedford_wear / div;
        }
    }
    if let Some(bump) = t.dc_susp_bump {
        if w.guild_suspicion >= 4 {
            dc += bump;
        }
    }
    roll(&mut w.rng, skill_value(&t.skill)) >= dc
}

fn resolve_beat(
    w: &mut WorldState,
    content: &ContentDb,
    beat: usize,
    choice: usize,
    events: &mut Vec<Event>,
) -> String {
    let Some(st) = w.run.beats.get(beat).and_then(|id| storylet(content, id)) else {
        return "The road continues, indifferent.".into();
    };
    let Some(ch): Option<&SChoice> = st.choices.get(choice) else {
        return "The road continues, indifferent.".into();
    };
    let ch = ch.clone();
    match &ch.test {
        Some(t) if !resolve_test(w, t) => {
            if let Some(fe) = &ch.fail_effects {
                apply_effects(w, fe, events);
            }
            subst(ch.fail_outcome.as_deref().unwrap_or("It does not go well."), w)
        }
        _ => {
            apply_effects(w, &ch.effects, events);
            subst(&ch.outcome, w)
        }
    }
}

fn deliver(w: &mut WorldState, content: &ContentDb, events: &mut Vec<Event>) {
    let payment = w.run.contract.payment_pence * w.run.payment_pct.max(0) / 100;
    w.cash_pence += payment;
    w.bedford_wear = (w.bedford_wear + 1).min(10);
    w.runs_completed += 1;
    narrate(w, events, format!("Counted twice, paid once: {}.", fmt_pence(payment)));

    let mut lines = vec![
        format!("Contract settled: {} received.", fmt_pence(payment)),
        format!(
            "Cash: {} when you pulled out, {} now.",
            fmt_pence(w.run.start_cash),
            fmt_pence(w.cash_pence)
        ),
        format!(
            "Fuel burned: {} L. Bedford wear: {}/10.",
            (w.run.start_fuel_ml - w.fuel_ml).max(0) / 1_000,
            w.bedford_wear
        ),
        format!(
            "Guild suspicion: {}/10{}",
            w.guild_suspicion,
            if w.guild_suspicion > w.run.start_suspicion { " — and climbing." } else { "." }
        ),
    ];
    for flag in &w.run.flags {
        if let Some(line) = foreshadow(flag) {
            lines.push(line.into());
        }
    }
    lines.push("Form 4B remains unfiled.".into());

    let _ = content;
    w.run.stage = RunStage::Summary { lines };
}

fn foreshadow(flag: &str) -> Option<&'static str> {
    Some(match flag {
        "gate_smashed" => "The Collective have your numberplate. That will mean something, later.",
        "bodged_belt" => "The fan belt is a stocking. The Bedford has not forgotten.",
        "helped_traveller" => "A seed merchant somewhere owes you a kindness.",
        "passed_traveller" => "A man on the verge of the road has your lights in his ledger.",
        "bribed_checkpoint" => "A cash box on the north road knows your face now.",
        "paid_carver" => "Carver's notebook has a tick against your name. Ticks accumulate.",
        "refused_carver" => "Carver's notebook has a different sort of mark against your name.",
        "took_salvage" => "Forty tins with no provenance sit in your stash. Tins talk.",
        "yielded_to_pulver" => "Pulver's boy will tell it as a victory. He'll be believed.",
        "ran_ambush" => "Somebody planned an evening around stopping you, and didn't.",
        "shaken_down" => "The men with the clean coats have your measure, and your shillings.",
        "took_drove_lane" => "The drove lane saw you. Drove lanes have friends.",
        "owes_form_4b" => "You are now formally obliged to apply for a form. On a Tuesday.",
        _ => return None,
    })
}

/// Ledger Close: fold run flags into the world, advance to the next dawn,
/// chalk up a fresh contract.
fn next_day(w: &mut WorldState, content: &ContentDb, events: &mut Vec<Event>) {
    let run_flags = std::mem::take(&mut w.run.flags);
    w.flags.extend(run_flags);

    w.tick += 1;
    while w.tick % PHASES_PER_DAY != 0 {
        w.tick += 1;
    }
    drift_diesel(w, events, -2, 2);

    let contract = gen_contract(&mut w.rng, content);
    w.run = RunState {
        contract,
        accepted: false,
        talked: false,
        stage: RunStage::Town,
        beats: Vec::new(),
        flags: BTreeSet::new(),
        payment_pct: 100,
        start_cash: w.cash_pence,
        start_fuel_ml: w.fuel_ml,
        start_wear: w.bedford_wear,
        start_suspicion: w.guild_suspicion,
    };
    narrate(
        w,
        events,
        format!("Dawn over Wychford, day {}. New chalk on the board.", w.day()),
    );
}

fn drift_diesel(w: &mut WorldState, events: &mut Vec<Event>, lo: i64, hi: i64) {
    let drift: i64 = w.rng.gen_range(lo..=hi);
    w.diesel_price_pence = (w.diesel_price_pence + drift).clamp(8, 48);
    events.push(Event::PriceDrift {
        tick: w.tick,
        good: "diesel".into(),
        settlement: "wychford".into(),
        price_pence: w.diesel_price_pence,
    });
}

/// Offline catch-up. Real days drift the economy and cool officialdom's
/// interest, but game days do not pass: Cobb does not age while you're away.
pub(crate) fn catch_up(w: &mut WorldState, days: u64, events: &mut Vec<Event>) {
    if days == 0 {
        return;
    }
    for _ in 0..days {
        drift_diesel(w, events, -3, 3);
        w.guild_suspicion = (w.guild_suspicion - 1).max(0);
    }
    narrate(
        w,
        events,
        format!(
            "While you were away: diesel in Wychford moved to {} the litre{}",
            fmt_pence(w.diesel_price_pence),
            if w.guild_suspicion == 0 {
                ", and nobody official said your name."
            } else {
                ". The Guild's interest cooled, a little."
            }
        ),
    );
}
