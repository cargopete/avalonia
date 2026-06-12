//! The Inquiry (Stage 5): a settlement panel convenes over something Cobb
//! actually did. Witness testimony is rendered from real NPC memories (gossip
//! and all); the contradiction detector compares a claim's asserts against
//! the canonical fact record — pure data comparison, no model in the loop.

use crate::memory::{chargeable, FactAttrs, FactRec};
use crate::*;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Claim {
    pub witness: String,
    pub witness_name: String,
    pub text: String,
    pub asserts: FactAttrs,
    /// "observation" | "gossip" — how the witness comes to know it.
    pub kind: String,
    pub challenged: Option<bool>, // Some(true) = contradiction established
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum InqStage {
    Summons,
    Brief,
    Hub,
    Testimony { claim_idx: usize },
    Verdict,
    Closed { lines: Vec<String> },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct InquiryState {
    pub case: FactRec,
    pub charge: String,
    pub witnesses: Vec<String>,
    pub claims: Vec<Claim>,
    pub questioned: BTreeSet<String>,
    pub credibility: i64,
    pub stage: InqStage,
}

/// At Ledger Close: does officialdom have grounds and appetite?
pub(crate) fn maybe_convene(w: &mut WorldState) -> bool {
    if w.inquiry.is_some() || w.guild_suspicion < 6 {
        return false;
    }
    // The gravest chargeable fact at least two people know of; one will do.
    let mut best: Option<(&FactRec, &'static str, i64)> = None;
    for f in &w.facts {
        let Some(charge) = chargeable(&f.attrs.action) else { continue };
        if w.flags.contains(&format!("tried_{}", f.fact_id)) {
            continue;
        }
        let holders: Vec<_> = w
            .memories
            .iter()
            .filter(|m| m.fact_id.as_deref() == Some(&f.fact_id))
            .collect();
        if holders.is_empty() {
            continue;
        }
        let weight = holders.iter().map(|m| m.importance).max().unwrap_or(0);
        if best.is_none() || weight > best.unwrap().2 {
            best = Some((f, charge, weight));
        }
    }
    let Some((fact, charge, _)) = best else { return false };
    let fact = fact.clone();
    let witnesses: Vec<String> = {
        let mut ws: Vec<String> = w
            .memories
            .iter()
            .filter(|m| m.fact_id.as_deref() == Some(&fact.fact_id))
            .map(|m| m.npc.clone())
            .collect();
        ws.sort();
        ws.dedup();
        ws.truncate(4);
        ws
    };
    w.flags.insert(format!("tried_{}", fact.fact_id));
    w.inquiry = Some(InquiryState {
        case: fact,
        charge: charge.into(),
        witnesses,
        claims: Vec::new(),
        questioned: BTreeSet::new(),
        credibility: 0,
        stage: InqStage::Summons,
    });
    true
}

fn npc_name<'c>(content: &'c ContentDb, id: &'c str) -> &'c str {
    content.npcs.iter().find(|n| n.id == id).map(|n| n.name.as_str()).unwrap_or(id)
}

pub(crate) fn scene(w: &WorldState, content: &ContentDb) -> Scene {
    let inq = w.inquiry.as_ref().expect("inquiry scene without inquiry");
    match &inq.stage {
        InqStage::Summons => Scene {
            title: "A Summons, in Triplicate".into(),
            body: vec![
                "It arrives before dawn, under the wiper blade like a parking notice: \
                 Form 7C, Petition for Determination of Fault, your name typed with \
                 two fingers and great conviction. The Joint Committee requests and \
                 requires your attendance at the Wychford corn exchange, today, in \
                 the matter described overleaf."
                    .into(),
                format!("Overleaf: {}.", inq.charge),
            ],
            choices: vec![Choice::on("Attend the hearing")],
        },
        InqStage::Brief => Scene {
            title: "The Corn Exchange — The Brief".into(),
            body: vec![
                "Trestle tables in a horseshoe, a jug of water nobody will touch, and \
                 a panel of three: a Collective captain, a Guild alderman, and Mr. \
                 Finch, who has brought his own stamps. You are handed the dossier — \
                 a carbon so faint it is less a document than a rumour of one."
                    .into(),
                format!("THE MATTER: {}.", inq.charge),
                format!(
                    "WITNESSES CALLED: {}.",
                    inq.witnesses
                        .iter()
                        .map(|id| npc_name(content, id))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
                "Their accounts will go into the record. The record, Finch observes, \
                 is the only thing in the room that cannot be wrong."
                    .into(),
            ],
            choices: vec![Choice::on("Begin the testimony")],
        },
        InqStage::Hub => {
            let mut body = vec!["The panel waits. The stenographer flexes her fingers.".into()];
            if !inq.claims.is_empty() {
                body.push("ON THE RECORD SO FAR:".into());
                for (i, c) in inq.claims.iter().enumerate() {
                    body.push(format!(
                        "{}. {} ({}): \"{}\"{}",
                        i + 1,
                        c.witness_name,
                        c.kind,
                        c.text,
                        match c.challenged {
                            Some(true) => " [CONTRADICTION ESTABLISHED]",
                            Some(false) => " [challenge failed]",
                            None => "",
                        }
                    ));
                }
            }
            let mut choices: Vec<Choice> = inq
                .witnesses
                .iter()
                .map(|id| {
                    if inq.questioned.contains(id) {
                        Choice::off(
                            format!("Question {}", npc_name(content, id)),
                            "Their account is on the record.",
                        )
                    } else {
                        Choice::on(format!("Question {}", npc_name(content, id)))
                    }
                })
                .collect();
            for (i, c) in inq.claims.iter().enumerate() {
                choices.push(match c.challenged {
                    None => Choice::on(format!(
                        "Challenge {}'s account against the record",
                        c.witness_name
                    )),
                    _ => Choice::off(
                        format!("Challenge {}'s account against the record", c.witness_name),
                        "Already put to the panel.",
                    ),
                });
                let _ = i;
            }
            choices.push(Choice::on("Let the panel proceed to a determination"));
            Scene { title: "The Corn Exchange — Testimony".into(), body, choices }
        }
        InqStage::Testimony { claim_idx } => {
            let c = &inq.claims[*claim_idx];
            Scene {
                title: format!("The Witness: {}", c.witness_name),
                body: vec![
                    format!("{} takes the chair and gives an account:", c.witness_name),
                    format!("\"{}\"", c.text),
                    match c.kind.as_str() {
                        "gossip" => "Pressed on how they come to know it, the witness \
                                     allows that they had it from somebody who had it \
                                     from somebody. The stenographer records this as \
                                     'directly'."
                            .into(),
                        _ => "The witness was there, and looks sorry for it.".to_string(),
                    },
                ],
                choices: vec![Choice::on("Return to the panel")],
            }
        }
        InqStage::Verdict => {
            let mut choices = vec![Choice::on("Accept the panel's finding and the fine (50s)")];
            choices.push(if inq.credibility >= 1 {
                Choice::on("Lay the matter at the door of Pulver's drivers")
            } else {
                Choice::off(
                    "Lay the matter at the door of Pulver's drivers",
                    "You'd need an established contradiction to hang it on.",
                )
            });
            choices.push(if inq.credibility >= 2 {
                Choice::on("Move to dismiss for want of evidence")
            } else {
                Choice::off(
                    "Move to dismiss for want of evidence",
                    "The record holds together too well. (Two contradictions needed.)",
                )
            });
            Scene {
                title: "The Determination".into(),
                body: vec![
                    "The panel confers in the manner of men agreeing about something \
                     they have not discussed. Finch uncaps a stamp. The room arrives \
                     at the part it was always going to arrive at."
                        .into(),
                    format!(
                        "Contradictions established: {}.",
                        inq.claims.iter().filter(|c| c.challenged == Some(true)).count()
                    ),
                ],
                choices,
            }
        }
        InqStage::Closed { lines } => Scene {
            title: "The Record".into(),
            body: lines.clone(),
            choices: vec![Choice::on("Step out into the morning")],
        },
    }
}

pub(crate) fn choose(w: &mut WorldState, idx: usize, content: &ContentDb, events: &mut Vec<Event>) {
    let Some(mut inq) = w.inquiry.take() else { return };
    match inq.stage.clone() {
        InqStage::Summons => inq.stage = InqStage::Brief,
        InqStage::Brief => inq.stage = InqStage::Hub,
        InqStage::Hub => {
            let n_wit = inq.witnesses.len();
            let n_claims = inq.claims.len();
            if idx < n_wit {
                // Question a witness: their best memory of the case becomes a claim.
                let npc = inq.witnesses[idx].clone();
                if let Some(m) = w
                    .memories
                    .iter()
                    .filter(|m| {
                        m.npc == npc && m.fact_id.as_deref() == Some(&inq.case.fact_id)
                    })
                    .max_by_key(|m| m.importance)
                {
                    inq.claims.push(Claim {
                        witness: npc.clone(),
                        witness_name: npc_name(content, &npc).to_string(),
                        text: m.text.clone(),
                        asserts: m.asserts.clone().unwrap_or_else(|| inq.case.attrs.clone()),
                        kind: m.kind.clone(),
                        challenged: None,
                    });
                    inq.questioned.insert(npc);
                    inq.stage = InqStage::Testimony { claim_idx: inq.claims.len() - 1 };
                }
            } else if idx < n_wit + n_claims {
                // Challenge claim against the canonical record.
                let ci = idx - n_wit;
                let contradiction = inq.claims[ci].asserts != inq.case.attrs;
                inq.claims[ci].challenged = Some(contradiction);
                if contradiction {
                    inq.credibility += 1;
                    narrate(
                        w,
                        events,
                        format!(
                            "The record is consulted. The record disagrees with {}. \
                             Finch stamps something with feeling.",
                            inq.claims[ci].witness_name
                        ),
                    );
                } else {
                    inq.credibility -= 1;
                    narrate(
                        w,
                        events,
                        format!(
                            "The record is consulted. The record agrees with {} to \
                             the letter. The panel looks at you as at a man who has \
                             wasted a stamp.",
                            inq.claims[ci].witness_name
                        ),
                    );
                }
            } else {
                inq.stage = InqStage::Verdict;
            }
        }
        InqStage::Testimony { .. } => inq.stage = InqStage::Hub,
        InqStage::Verdict => {
            let lines = match idx {
                0 => {
                    let fine = 600.min(w.cash_pence);
                    w.cash_pence -= fine;
                    w.guild_suspicion = (w.guild_suspicion - 3).max(0);
                    vec![
                        format!(
                            "FINDING: fault determined. Fine of {} paid into the \
                             provisional fund, receipted in triplicate.",
                            fmt_pence(fine)
                        ),
                        "The matter is closed, the file is opened, and both will \
                         outlive everyone in this room."
                            .into(),
                    ]
                }
                1 => {
                    w.guild_suspicion = (w.guild_suspicion - 4).max(0);
                    let v = w.factions.entry("carver".into()).or_insert(0);
                    *v = (*v - 1).clamp(-5, 5);
                    w.flags.insert("framed_pulver".into());
                    vec![
                        "FINDING: fault determined against persons unknown, believed \
                         to be drivers in the employ of one Pulver. A warrant is \
                         typed. Somewhere on the night roads, a young man in a scarf \
                         has acquired a paperwork problem."
                            .into(),
                        "You walk out lighter. The lightness has a price that hasn't \
                         been named yet."
                            .into(),
                    ]
                }
                _ => {
                    w.guild_suspicion = (w.guild_suspicion - 5).max(0);
                    let v = w.factions.entry("collective".into()).or_insert(0);
                    *v = (*v + 1).clamp(-5, 5);
                    vec![
                        "FINDING: dismissed for want of evidence. Finch pronounces \
                         the words like a eulogy for the record's good name. The \
                         Collective captain shakes your hand, once, as if testing \
                         a gate for soundness."
                            .into(),
                        "You are, officially, a man about whom nothing can be proved. \
                         There are worse reputations on these roads."
                            .into(),
                    ]
                }
            };
            inq.stage = InqStage::Closed { lines };
        }
        InqStage::Closed { .. } => {
            // Step out: the inquiry dissolves; the town day proceeds.
            narrate(
                w,
                events,
                "The corn exchange empties. Wychford pretends it wasn't listening.",
            );
            w.inquiry = None;
            return;
        }
    }
    w.inquiry = Some(inq);
}
