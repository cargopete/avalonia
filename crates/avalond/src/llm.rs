//! avalond ↔ orchestrator glue: prompt assembly, memory retrieval, the
//! interactive dialogue path, and the idle-lane job worker (DR-5).
//!
//! Canon discipline: nothing here mutates the world directly. LLM output
//! either ships to the player as cosmetic text, or becomes a recorded-oracle
//! Command (RecordChat / AddReflection / RewriteMemory) applied through the
//! same `apply()` path as player choices.

use crate::{apply, App};
use avalon_content::Npc;
use avalon_orchestrator::{cosine, Lane};
use avalon_sim::{fmt_pence, Command, MemoryEntry, WorldState};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

pub const WORLD_BIBLE: &str = "\
SETTING: Avalon — a 1930s-flavoured English district after a quiet collapse of \
central authority. Market towns (Wychford, Draycote, Millhaven, Stane End, \
Ferrier Bridge) run themselves through committees, guilds and militias. The \
Wychford Traders' Guild moves goods; The Collective Force runs barriers and \
stamps forms; Sal Carver 'maintains' roads for tolls. Fuel is scarce, paperwork \
is sacred, nothing is quite legal. COBB is a truck driver who hauls cargo — \
some of it licensed — in a battered Bedford TK.\n\
VOICE: dry English understatement, 1930s period. Bureaucratic comedy played \
straight. No modern slang, no Americanisms, no melodrama. Short sentences.\n\
HARD RULES: never invent place names, prices, or people not given to you. \
Do not narrate actions; speak only the character's words.";

fn settlement_name<'a>(app: &'a App, id: &'a str) -> &'a str {
    app.content
        .settlements
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.name.as_str())
        .unwrap_or(id)
}

// ----------------------------------------------------------- retrieval

/// Smallville scoring, all alphas 1: recency + importance + relevance.
/// Relevance uses cached embeddings only — the interactive lane never waits
/// for an embed; missing vectors just score 0 and the idle lane backfills.
fn retrieve(
    conn: &Connection,
    w: &WorldState,
    npc: &str,
    query_vec: Option<&[f32]>,
    k: usize,
) -> Vec<MemoryEntry> {
    let mut scored: Vec<(f32, &MemoryEntry)> = w
        .memories
        .iter()
        .filter(|m| m.npc == npc)
        .map(|m| {
            let age = (w.tick.saturating_sub(m.tick)) as f32;
            let recency = (-age / 40.0).exp();
            let importance = m.importance as f32 / 10.0;
            let relevance = match query_vec {
                Some(q) => embedding_of(conn, m.id)
                    .map(|v| cosine(q, &v))
                    .unwrap_or(0.0),
                None => 0.0,
            };
            (recency + importance + relevance, m)
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(k).map(|(_, m)| m.clone()).collect()
}

fn embedding_of(conn: &Connection, memory_id: u64) -> Option<Vec<f32>> {
    let blob: Vec<u8> = conn
        .query_row(
            "SELECT vec FROM embeddings WHERE memory_id = ?1",
            [memory_id as i64],
            |r| r.get(0),
        )
        .ok()?;
    Some(
        blob.chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    )
}

fn store_embedding(conn: &Connection, memory_id: u64, vec: &[f32]) {
    let blob: Vec<u8> = vec.iter().flat_map(|f| f.to_le_bytes()).collect();
    let _ = conn.execute(
        "INSERT INTO embeddings(memory_id, vec) VALUES(?1, ?2)
         ON CONFLICT(memory_id) DO UPDATE SET vec = ?2",
        rusqlite::params![memory_id as i64, blob],
    );
}

// ----------------------------------------------------- interactive: dialogue

fn dialogue_system(_app: &App, npc: &Npc, memories: &[MemoryEntry], w: &WorldState) -> String {
    let mem_lines: String = memories
        .iter()
        .map(|m| format!("- ({}) {}\n", m.kind, m.text))
        .collect();
    let scene = format!(
        "Right now: {} at the Wychford depot, {} of day {}. Cobb's standing with \
         the Guild: suspicion {}/10. Today's board: {} of {} to {}, {} on delivery{}.",
        npc.name,
        w.phase_name(),
        w.day(),
        w.guild_suspicion,
        w.run.contract.qty_desc,
        w.run.contract.cargo,
        w.run.contract.dest_name,
        fmt_pence(w.run.contract.payment_pence),
        if w.run.contract.illicit { " (no Form 4B exists for it)" } else { "" }
    );
    format!(
        "{WORLD_BIBLE}\n\nYOU ARE: {}, {}. {}\n\nWHAT YOU REMEMBER (yours alone; \
         do not invent more):\n{}\n{}\n\nReply as {} in one or two sentences, in \
         period voice, words only. Never repeat a line you have said before; \
         answer the actual question.",
        npc.name, npc.role, npc.persona, mem_lines, scene, npc.name
    )
}

fn bark(npc: &Npc, salt: usize) -> String {
    if npc.barks.is_empty() {
        return format!("{} says nothing you could put on a form.", npc.name)
    }
    npc.barks[salt % npc.barks.len()].clone()
}

/// The full interactive path: retrieve → generate → validate → fallback,
/// then record the exchange into canon via RecordChat.
pub async fn npc_reply(app: &Arc<App>, npc_id: &str, player_text: &str) -> (String, bool) {
    let Some(npc) = app.content.npcs.iter().find(|n| n.id == npc_id).cloned() else {
        return ("Nobody of that name works this depot.".into(), true);
    };
    let query_vec = app.orch.embed(player_text).await;
    let (system, salt) = {
        let w = app.world.lock().unwrap();
        let conn = app.db.lock().unwrap();
        let memories = retrieve(&conn, &w, npc_id, query_vec.as_deref(), 5);
        (dialogue_system(app, &npc, &memories, &w), w.tick as usize + player_text.len())
    };
    let reply = app.orch.say(Lane::Interactive, &system, player_text).await;
    let (line, fallback) = match reply {
        Some(text) => (text, false),
        None => (bark(&npc, salt), true),
    };
    // Recorded oracle: the conversation happened; the NPC keeps a residue.
    let summary = format!(
        "Cobb said: '{}'. I said: '{}'.",
        truncate(player_text, 90),
        truncate(&line, 120)
    );
    apply(app, Command::RecordChat { npc: npc_id.into(), summary });
    (line, fallback)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let cut: String = s.chars().take(n).collect();
        format!("{cut}…")
    }
}

// ------------------------------------------------------------- idle lane

pub fn enqueue(conn: &Connection, kind: &str, payload: Value) {
    let _ = conn.execute(
        "INSERT INTO jobs(kind, payload, state) VALUES(?1, ?2, 'queued')",
        rusqlite::params![kind, payload.to_string()],
    );
}

/// Called from apply() after every step: new memories get embed jobs, fresh
/// gossip gets a rewording pass, and NPCs whose new memories matter enough
/// get a reflection (Park-style importance-sum trigger).
pub fn enqueue_for_new_memories(conn: &Connection, w: &WorldState, watermark: u64) {
    let mut per_npc: std::collections::BTreeMap<&str, i64> = Default::default();
    for m in w.memories.iter().filter(|m| m.id >= watermark) {
        enqueue(conn, "embed", json!({ "memory_id": m.id, "text": m.text }));
        if m.kind == "gossip" {
            enqueue(conn, "gossip_rewrite", json!({ "memory_id": m.id }));
        }
        if m.kind != "chat" {
            *per_npc.entry(m.npc.as_str()).or_insert(0) += m.importance;
        }
    }
    for (npc, sum) in per_npc {
        if sum >= 8 {
            enqueue(conn, "reflection", json!({ "npc": npc }));
        }
    }
}

fn pop_job(conn: &Connection) -> Option<(i64, String, Value)> {
    let row = conn
        .query_row(
            "SELECT id, kind, payload FROM jobs
             WHERE state = 'queued' AND attempts < 2
             ORDER BY id LIMIT 1",
            [],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .ok()?;
    let _ = conn.execute(
        "UPDATE jobs SET state = 'running', attempts = attempts + 1 WHERE id = ?1",
        [row.0],
    );
    Some((row.0, row.1, serde_json::from_str(&row.2).unwrap_or(Value::Null)))
}

fn finish_job(conn: &Connection, id: i64, ok: bool) {
    let state = if ok { "done" } else { "queued" }; // retried once, then attempts cap parks it
    let _ = conn.execute("UPDATE jobs SET state = ?1 WHERE id = ?2", rusqlite::params![state, id]);
}

/// The idle worker: chews queued jobs whenever the interactive lane has been
/// quiet. Nothing it does is load-bearing — a dead model just means a plainer
/// world (DR-5: never block play).
pub async fn idle_worker(app: Arc<App>) {
    loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        if !app.orch.idle_clear() {
            continue;
        }
        let job = {
            let conn = app.db.lock().unwrap();
            pop_job(&conn)
        };
        let Some((id, kind, payload)) = job else { continue };
        let ok = run_job(&app, &kind, &payload).await;
        let conn = app.db.lock().unwrap();
        finish_job(&conn, id, ok);
    }
}

async fn run_job(app: &Arc<App>, kind: &str, p: &Value) -> bool {
    match kind {
        "embed" => {
            let (Some(mid), Some(text)) = (p["memory_id"].as_u64(), p["text"].as_str()) else {
                return true; // malformed: drop
            };
            match app.orch.embed(text).await {
                Some(vec) => {
                    let conn = app.db.lock().unwrap();
                    store_embedding(&conn, mid, &vec);
                    true
                }
                None => false,
            }
        }
        "reflection" => {
            let Some(npc_id) = p["npc"].as_str() else { return true };
            let Some(npc) = app.content.npcs.iter().find(|n| n.id == npc_id).cloned() else {
                return true;
            };
            let recent: Vec<String> = {
                let w = app.world.lock().unwrap();
                let mut ms: Vec<_> =
                    w.memories.iter().filter(|m| m.npc == npc_id).collect();
                ms.sort_by_key(|m| std::cmp::Reverse(m.tick));
                ms.into_iter().take(8).map(|m| format!("- {}", m.text)).collect()
            };
            if recent.is_empty() {
                return true;
            }
            let system = format!(
                "{WORLD_BIBLE}\n\nYOU ARE: {}, {}. {}",
                npc.name, npc.role, npc.persona
            );
            let user = format!(
                "Your recent knowledge of Cobb:\n{}\n\nState, in one sentence in \
                 your own voice, the settled opinion you now hold about Cobb. No \
                 preamble.",
                recent.join("\n")
            );
            match app.orch.say(Lane::Idle, &system, &user).await {
                Some(text) => {
                    apply(app, Command::AddReflection { npc: npc_id.into(), text, importance: 6 });
                    true
                }
                None => false,
            }
        }
        "gossip_rewrite" => {
            let Some(mid) = p["memory_id"].as_u64() else { return true };
            let (npc_id, text, place, actor) = {
                let w = app.world.lock().unwrap();
                let Some(m) = w.memories.iter().find(|m| m.id == mid) else {
                    return true; // memory aged out: drop
                };
                let a = m.asserts.clone();
                (
                    m.npc.clone(),
                    m.text.clone(),
                    a.as_ref().map(|a| settlement_name(app, &a.location).to_string()),
                    a.as_ref().map(|a| a.actor.clone()),
                )
            };
            let Some(npc) = app.content.npcs.iter().find(|n| n.id == npc_id).cloned() else {
                return true;
            };
            let who = match actor.as_deref() {
                Some("cobb") => "Cobb".to_string(),
                Some(other) => other.to_string(),
                None => "Cobb".into(),
            };
            let place = place.unwrap_or_default();
            let system = format!(
                "{WORLD_BIBLE}\n\nYOU ARE: {}, {}. {}",
                npc.name, npc.role, npc.persona
            );
            let user = format!(
                "Retell this rumour as you'd pass it on over the bar, in one or two \
                 sentences. You MUST keep who it concerns ({who}) and the place \
                 ({place}) exactly as given — embellish everything else:\n\"{text}\""
            );
            match app.orch.say(Lane::Idle, &system, &user).await {
                Some(new_text) => {
                    // Fact-guard (stage 2 of the pipeline): the distorted-but-
                    // canonical WHO and WHERE must survive the retelling.
                    let okay = new_text.contains(&place)
                        && (new_text.contains(&who)
                            || new_text.to_lowercase().contains(&who.to_lowercase()));
                    if okay {
                        apply(app, Command::RewriteMemory { memory_id: mid, text: new_text });
                    }
                    true // even a failed guard is a completed job; original text stands
                }
                None => false,
            }
        }
        "digest" => {
            let Some(text) = p["text"].as_str() else { return true };
            let user = format!(
                "Rewrite this ledger note as two dry margin-note sentences in the \
                 period voice, keeping every number and place name exactly:\n{text}"
            );
            match app.orch.say(Lane::Idle, WORLD_BIBLE, &user).await {
                Some(rewritten) => {
                    // Display-only: broadcast to the log, never into canon.
                    let _ = app.tx.send(
                        json!({ "kind": "narration", "tick": 0, "text": rewritten }).to_string(),
                    );
                    true
                }
                None => false,
            }
        }
        _ => true,
    }
}
