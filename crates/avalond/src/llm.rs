//! avalond ↔ orchestrator glue: the fixer dialogue path, mission-scoped.
//!
//! Ephemeral game: no persistent memory, no idle lane. The fixer answers in
//! character about the current run, with a short in-session chat log for
//! coherence that evaporates when the mission ends. Canon is untouched —
//! dialogue is cosmetic; if the model is down, authored barks ship.

use crate::App;
use avalon_content::Npc;
use avalon_orchestrator::Lane;
use avalon_sim::{fmt_pence, Mission};
use std::sync::Arc;

pub const WORLD_BIBLE: &str = "\
SETTING: Avalon — a 1930s-flavoured English district after a quiet collapse of \
central authority. Market towns run themselves through committees, guilds and \
militias. The Collective Force runs barriers and stamps forms; Sal Carver \
'maintains' roads for tolls; Pulver's lot run rival trucks. Fuel is scarce, \
paperwork is sacred, nothing is quite legal. COBB is a haulier who runs cargo \
in a battered Bedford TK, one job at a time.\n\
VOICE: dry English understatement, 1930s period. Bureaucratic comedy played \
straight. No modern slang, no Americanisms, no melodrama. Short sentences.\n\
HARD RULES: never invent place names, prices, or people not given to you. \
Speak only the character's words — no narration, no stage directions.";

fn dialogue_system(npc: &Npc, m: &Mission, chat: &[(String, String)]) -> String {
    let job = format!(
        "Tonight's job: {} of {} across {} to {}, {} on delivery{}. The Bedford's \
         at wear {}/10; heat on Cobb is {}/10.",
        m.qty_desc,
        m.cargo,
        m.terrain_name,
        m.dest_name,
        fmt_pence(m.reward_pence),
        if m.illicit { " (no Form 4B exists for it)" } else { "" },
        m.wear,
        m.heat,
    );
    let history = if chat.is_empty() {
        String::new()
    } else {
        let mut h = String::from("\nThe conversation so far:\n");
        for (you, them) in chat.iter().rev().take(3).rev() {
            h.push_str(&format!("Cobb: {you}\n{}: {them}\n", npc.name));
        }
        h
    };
    format!(
        "{WORLD_BIBLE}\n\nYOU ARE: {}, {}. {}\n\n{job}{history}\n\nReply as {} in one \
         or two sentences, in period voice, words only. Answer the actual question; \
         never repeat an earlier line.",
        npc.name, npc.role, npc.persona, npc.name
    )
}

fn bark(npc: &Npc, salt: usize) -> String {
    if npc.barks.is_empty() {
        format!("{} says nothing you could put on a form.", npc.name)
    } else {
        npc.barks[salt % npc.barks.len()].clone()
    }
}

/// Retrieve → generate → validate → authored-bark fallback. Records the
/// exchange in the App's mission-scoped chat log for coherence.
pub async fn npc_reply(app: &Arc<App>, npc_id: &str, player_text: &str) -> (String, bool) {
    let Some(npc) = app.content.npcs.iter().find(|n| n.id == npc_id).cloned() else {
        return ("Nobody of that name works this depot.".into(), true);
    };
    let (system, salt) = {
        let guard = app.mission.lock().unwrap();
        let Some(m) = guard.as_ref() else {
            return ("The depot's empty. Start a run first.".into(), true);
        };
        let chat = app.chat.lock().unwrap();
        (dialogue_system(&npc, m, &chat), m.heat as usize + player_text.len())
    };
    let (line, fallback) = match app.orch.say(Lane::Interactive, &system, player_text).await {
        Some(text) => (text, false),
        None => (bark(&npc, salt), true),
    };
    {
        let mut chat = app.chat.lock().unwrap();
        chat.push((player_text.to_string(), line.clone()));
        if chat.len() > 8 {
            let drop = chat.len() - 8;
            chat.drain(0..drop);
        }
    }
    (line, fallback)
}
