//! avalond — the local daemon: axum API, SSE, SQLite save, sessions, jobs
//! (RFC-AVL-001 DR-1/DR-4/DR-5).

mod llm;

use avalon_orchestrator::{LlmConfig, Orchestrator};
use avalon_sim::{scene, step, Command, ContentDb, Event, Scene, WorldState};
use axum::extract::State;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

const BIND: &str = "127.0.0.1:4747";
const DEFAULT_SEED: u64 = 0xC0BB;

/// Schema migrations, applied in order; PRAGMA user_version tracks progress.
/// Append-only: never edit a shipped entry (DR-4).
const MIGRATIONS: &[&str] = &[
    // v1: meta (world snapshot, schema bookkeeping) + canonical event log
    "CREATE TABLE meta(key TEXT PRIMARY KEY, val TEXT NOT NULL);
     CREATE TABLE events(
         id INTEGER PRIMARY KEY,
         tick INTEGER NOT NULL,
         kind TEXT NOT NULL,
         payload TEXT NOT NULL
     );
     CREATE INDEX idx_events_tick ON events(tick);",
    // v2: idle-lane job queue + embedding cache (Stage 3/4)
    "CREATE TABLE jobs(
         id INTEGER PRIMARY KEY,
         kind TEXT NOT NULL,
         payload TEXT NOT NULL,
         state TEXT NOT NULL DEFAULT 'queued',
         attempts INTEGER NOT NULL DEFAULT 0,
         created_at INTEGER DEFAULT (unixepoch())
     );
     CREATE INDEX idx_jobs_state ON jobs(state);
     CREATE TABLE embeddings(memory_id INTEGER PRIMARY KEY, vec BLOB NOT NULL);",
];

pub(crate) struct App {
    content: Arc<ContentDb>,
    orch: Arc<Orchestrator>,
    world: Mutex<WorldState>,
    db: Mutex<Connection>,
    tx: broadcast::Sender<String>,
}

#[derive(Serialize)]
struct StateView {
    tick: u64,
    day: u64,
    phase: &'static str,
    cash_pence: i64,
    fuel_l: i64,
    diesel_price_pence: i64,
    bedford_wear: i64,
    guild_suspicion: i64,
    runs_completed: u32,
    cargo: Option<String>,
}

#[derive(Serialize)]
struct View {
    state: StateView,
    scene: Scene,
}

#[derive(Deserialize)]
struct ChooseReq {
    idx: usize,
}

#[derive(Deserialize)]
struct SayReq {
    npc: String,
    text: String,
}

impl StateView {
    fn of(w: &WorldState) -> Self {
        Self {
            tick: w.tick,
            day: w.day(),
            phase: w.phase_name(),
            cash_pence: w.cash_pence,
            fuel_l: w.fuel_ml / 1_000,
            diesel_price_pence: w.diesel_price_pence,
            bedford_wear: w.bedford_wear,
            guild_suspicion: w.guild_suspicion,
            runs_completed: w.runs_completed,
            cargo: w
                .run
                .accepted
                .then(|| format!("{} → {}", w.run.contract.cargo, w.run.contract.dest_name)),
        }
    }
}

fn view_of(w: &WorldState, c: &ContentDb) -> View {
    View { state: StateView::of(w), scene: scene(w, c) }
}

fn epoch_day() -> i64 {
    (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before 1970")
        .as_secs()
        / 86_400) as i64
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let target = i as i64 + 1;
        if version < target {
            conn.execute_batch(sql)?;
            conn.pragma_update(None, "user_version", target)?;
        }
    }
    Ok(())
}

fn meta_get(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT val FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
}

fn meta_set(conn: &Connection, key: &str, val: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO meta(key, val) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET val = ?2",
        [key, val],
    )?;
    Ok(())
}

fn load_world(conn: &Connection, content: &ContentDb) -> rusqlite::Result<WorldState> {
    Ok(match meta_get(conn, "world")? {
        Some(json) => match serde_json::from_str(&json) {
            Ok(w) => w,
            Err(e) => {
                // Stage 1 only: snapshot-shape changes start a fresh world.
                // Real saves get forward-migrations before anything ships.
                eprintln!("avalond: snapshot incompatible ({e}); starting a fresh world");
                WorldState::new(DEFAULT_SEED, content)
            }
        },
        None => WorldState::new(DEFAULT_SEED, content),
    })
}

fn persist(conn: &Connection, world: &WorldState, events: &[Event]) -> rusqlite::Result<()> {
    let json = serde_json::to_string(world).expect("WorldState is always serializable");
    meta_set(conn, "world", &json)?;
    meta_set(conn, "last_epoch_day", &epoch_day().to_string())?;
    for ev in events {
        let value = serde_json::to_value(ev).expect("Event is always serializable");
        let kind = value["kind"].as_str().unwrap_or("unknown").to_string();
        conn.execute(
            "INSERT INTO events(tick, kind, payload) VALUES(?1, ?2, ?3)",
            rusqlite::params![world.tick as i64, kind, value.to_string()],
        )?;
    }
    Ok(())
}

pub(crate) fn apply(app: &App, cmd: Command) -> View {
    let mut w = app.world.lock().unwrap();
    let memory_watermark = w.next_memory_id;
    let (next, events) = step(&w, &cmd, &app.content);
    {
        let conn = app.db.lock().unwrap();
        persist(&conn, &next, &events).expect("persist failed");
        llm::enqueue_for_new_memories(&conn, &next, memory_watermark);
    }
    for ev in &events {
        let _ = app.tx.send(serde_json::to_string(ev).unwrap());
    }
    *w = next;
    view_of(&w, &app.content)
}

async fn get_view(State(app): State<Arc<App>>) -> Json<View> {
    let w = app.world.lock().unwrap();
    Json(view_of(&w, &app.content))
}

async fn choose(State(app): State<Arc<App>>, Json(req): Json<ChooseReq>) -> Json<View> {
    Json(apply(&app, Command::Choose { idx: req.idx }))
}

async fn say(
    State(app): State<Arc<App>>,
    Json(req): Json<SayReq>,
) -> Json<serde_json::Value> {
    let text = req.text.trim().to_string();
    if text.is_empty() || text.len() > 400 {
        return Json(serde_json::json!({ "line": "…", "fallback": true }));
    }
    let (line, fallback) = llm::npc_reply(&app, &req.npc, &text).await;
    Json(serde_json::json!({ "npc": req.npc, "line": line, "fallback": fallback }))
}

async fn status(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let jobs: i64 = {
        let conn = app.db.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM jobs WHERE state = 'queued' AND attempts < 2", [], |r| r.get(0))
            .unwrap_or(0)
    };
    Json(serde_json::json!({ "llm": app.orch.available().await, "queued_jobs": jobs }))
}

async fn debug_world(State(app): State<Arc<App>>) -> Json<WorldState> {
    Json(app.world.lock().unwrap().clone())
}

async fn stream(
    State(app): State<Arc<App>>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = app.tx.subscribe();
    let stream = BroadcastStream::new(rx)
        .filter_map(|msg| msg.ok().map(|m| Ok(SseEvent::default().data(m))));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[tokio::main]
async fn main() {
    let save_path: PathBuf = std::env::var("AVALON_SAVE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("saves/dev.db"));
    if let Some(dir) = save_path.parent() {
        std::fs::create_dir_all(dir).expect("create saves dir");
    }
    let conn = Connection::open(&save_path).expect("open save db");
    conn.pragma_update(None, "journal_mode", "WAL").expect("WAL");
    migrate(&conn).expect("migrations");
    let content_dir: PathBuf = std::env::var("AVALON_CONTENT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("content"));
    let content = Arc::new(avalon_content::load(&content_dir).expect("content validates"));
    println!(
        "avalond: content loaded — {} storylets, {} settlements, {} npcs",
        content.storylets.len(),
        content.settlements.len(),
        content.npcs.len()
    );
    let mut world = load_world(&conn, &content).expect("load world");

    // Delta-time: real days away drift the economy (Command::CatchUp keeps
    // the sim pure; the wall clock stays out here in the impure shell).
    let away_days = meta_get(&conn, "last_epoch_day")
        .expect("read last_epoch_day")
        .and_then(|s| s.parse::<i64>().ok())
        .map(|last| (epoch_day() - last).max(0) as u64)
        .unwrap_or(0);
    if away_days > 0 {
        let (next, events) = step(&world, &Command::CatchUp { days: away_days }, &content);
        persist(&conn, &next, &events).expect("persist catch-up");
        // The digest job rewrites the templated note in voice, later, idly.
        if let Some(Event::Narration { text, .. }) =
            events.iter().rev().find(|e| matches!(e, Event::Narration { .. }))
        {
            llm::enqueue(&conn, "digest", serde_json::json!({ "text": text }));
        }
        world = next;
        println!("avalond: caught up {away_days} day(s) away");
    }

    println!(
        "avalond: world at tick {} (day {}, {}), {} runs done, save {}",
        world.tick,
        world.day(),
        world.phase_name(),
        world.runs_completed,
        save_path.display()
    );

    let (tx, _) = broadcast::channel(256);
    let orch = Arc::new(Orchestrator::new(LlmConfig {
        banned_phrases: content.banned_phrases.clone(),
        ..Default::default()
    }));
    let app = Arc::new(App {
        content,
        orch,
        world: Mutex::new(world),
        db: Mutex::new(conn),
        tx,
    });

    // The idle lane: reflections, gossip rewording, embeddings, digests.
    tokio::spawn(llm::idle_worker(app.clone()));
    println!(
        "avalond: llm at {} ({})",
        app.orch.cfg.host,
        if app.orch.available().await { "reachable" } else { "unreachable — fallbacks only" }
    );

    let router = Router::new()
        .route("/api/view", get(get_view))
        .route("/api/choose", post(choose))
        .route("/api/say", post(say))
        .route("/api/status", get(status))
        .route("/api/debug/world", get(debug_world))
        .route("/api/stream", get(stream))
        .with_state(app);

    let listener = tokio::net::TcpListener::bind(BIND).await.expect("bind");
    println!("avalond: listening on http://{BIND}");
    axum::serve(listener, router).await.expect("serve");
}
