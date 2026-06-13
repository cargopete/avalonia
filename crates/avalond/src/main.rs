//! avalond — the local daemon (RFC-AVL-001 DR-1/DR-5), reshaped for ephemeral
//! single-mission play: one in-memory `Mission` at a time, no save/resume.
//! `/api/new` rolls a fresh run; closing the tab abandons it. SQLite keeps
//! only a scoreboard of finished runs.

mod llm;

use avalon_orchestrator::{LlmConfig, Orchestrator};
use avalon_sim::{scene, step, Command, ContentDb, Event, Mission, Outcome, Scene};
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
use tower_http::services::{ServeDir, ServeFile};

fn bind_addr() -> String {
    std::env::var("AVALON_BIND").unwrap_or_else(|_| "127.0.0.1:4747".into())
}

/// Append-only migrations; PRAGMA user_version tracks progress (DR-4).
const MIGRATIONS: &[&str] = &[
    // v1/v2 (persistent-world era) are retired; recreate them as no-ops so a
    // fresh DB lands on the same user_version as upgraded ones.
    "SELECT 1;",
    "SELECT 1;",
    // v3: the scoreboard — the only thing that outlives a mission.
    "CREATE TABLE IF NOT EXISTS scores(
         id INTEGER PRIMARY KEY,
         outcome TEXT NOT NULL,
         reason TEXT,
         haul_pence INTEGER NOT NULL DEFAULT 0,
         terrain TEXT NOT NULL,
         cargo TEXT NOT NULL,
         seed INTEGER NOT NULL,
         finished_at INTEGER NOT NULL
     );",
];

pub(crate) struct App {
    content: Arc<ContentDb>,
    orch: Arc<Orchestrator>,
    /// The one live run, or None at the start screen.
    mission: Mutex<Option<Mission>>,
    /// Mission-scoped fixer chat log (cleared on new/abandon).
    chat: Mutex<Vec<(String, String)>>,
    db: Mutex<Connection>,
    tx: broadcast::Sender<String>,
}

// --------------------------------------------------------------- view DTOs

#[derive(Serialize)]
struct MissionView {
    terrain: String,
    terrain_name: String,
    cargo: String,
    qty_desc: String,
    dest_name: String,
    illicit: bool,
    reward_pence: i64,
    cash_pence: i64,
    fuel_l: i64,
    fuel_pct: i64,
    max_fuel_l: i64,
    wear: i64,
    heat: i64,
    leg: usize,
    legs_total: usize,
    fixer: String,
    antagonist: String,
    outcome: &'static str,
    reason: Option<String>,
    haul_pence: Option<i64>,
    scene: Scene,
}

#[derive(Serialize, Default)]
struct Scores {
    played: i64,
    won: i64,
    lost: i64,
    best_haul_pence: i64,
}

#[derive(Serialize)]
struct View {
    active: bool,
    mission: Option<MissionView>,
    scores: Scores,
}

impl MissionView {
    fn of(m: &Mission, content: &ContentDb) -> Self {
        let (outcome, reason, haul) = match &m.outcome {
            Outcome::InProgress => ("in_progress", None, None),
            Outcome::Won { haul_pence, .. } => ("won", None, Some(*haul_pence)),
            Outcome::Lost { reason, .. } => ("lost", Some(reason.clone()), None),
        };
        Self {
            terrain: m.terrain.clone(),
            terrain_name: m.terrain_name.clone(),
            cargo: m.cargo.clone(),
            qty_desc: m.qty_desc.clone(),
            dest_name: m.dest_name.clone(),
            illicit: m.illicit,
            reward_pence: m.reward_pence,
            cash_pence: m.cash_pence,
            fuel_l: m.fuel_l(),
            fuel_pct: m.fuel_pct(),
            max_fuel_l: m.max_fuel_ml / 1_000,
            wear: m.wear,
            heat: m.heat,
            leg: m.leg,
            legs_total: m.legs_total,
            fixer: m.fixer.clone(),
            antagonist: m.antagonist.clone(),
            outcome,
            reason,
            haul_pence: haul,
            scene: scene(m, content),
        }
    }
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn fresh_seed() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0xC0BB)
}

// ----------------------------------------------------------------- db

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

fn read_scores(conn: &Connection) -> Scores {
    conn.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(outcome = 'won'), 0),
                COALESCE(SUM(outcome = 'lost'), 0),
                COALESCE(MAX(haul_pence), 0)
         FROM scores",
        [],
        |r| Ok(Scores { played: r.get(0)?, won: r.get(1)?, lost: r.get(2)?, best_haul_pence: r.get(3)? }),
    )
    .unwrap_or_default()
}

/// Record a finished mission once. Idempotency: callers null the mission's
/// outcome path by only recording on the transition into Over.
fn record_score(conn: &Connection, m: &Mission) {
    let (outcome, reason, haul) = match &m.outcome {
        Outcome::Won { haul_pence, .. } => ("won", None, *haul_pence),
        Outcome::Lost { reason, .. } => ("lost", Some(reason.as_str()), 0),
        Outcome::InProgress => return,
    };
    let _ = conn.execute(
        "INSERT INTO scores(outcome, reason, haul_pence, terrain, cargo, seed, finished_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![outcome, reason, haul, m.terrain, m.cargo, m.seed as i64, now_secs()],
    );
}

// --------------------------------------------------------------- handlers

fn view_payload(app: &App) -> View {
    let guard = app.mission.lock().unwrap();
    let scores = {
        let conn = app.db.lock().unwrap();
        read_scores(&conn)
    };
    match guard.as_ref() {
        Some(m) => View { active: true, mission: Some(MissionView::of(m, &app.content)), scores },
        None => View { active: false, mission: None, scores },
    }
}

async fn get_view(State(app): State<Arc<App>>) -> Json<View> {
    Json(view_payload(&app))
}

async fn new_mission(State(app): State<Arc<App>>) -> Json<View> {
    {
        let mut guard = app.mission.lock().unwrap();
        *guard = Some(Mission::new(fresh_seed(), &app.content));
        app.chat.lock().unwrap().clear();
    }
    Json(view_payload(&app))
}

async fn abandon(State(app): State<Arc<App>>) {
    *app.mission.lock().unwrap() = None;
    app.chat.lock().unwrap().clear();
}

#[derive(Deserialize)]
struct ChooseReq {
    idx: usize,
}

async fn choose(State(app): State<Arc<App>>, Json(req): Json<ChooseReq>) -> Json<View> {
    {
        let mut guard = app.mission.lock().unwrap();
        if let Some(m) = guard.as_ref() {
            let was_over = m.outcome.is_over();
            let (next, events) = step(m, &Command::Choose { idx: req.idx }, &app.content);
            for ev in &events {
                let Event::Narration { text } = ev;
                let _ = app.tx.send(text.clone());
            }
            // Record the score exactly once: on the transition into Over.
            if !was_over && next.outcome.is_over() {
                let conn = app.db.lock().unwrap();
                record_score(&conn, &next);
            }
            *guard = Some(next);
        }
    }
    Json(view_payload(&app))
}

#[derive(Deserialize)]
struct SayReq {
    npc: String,
    text: String,
}

async fn say(State(app): State<Arc<App>>, Json(req): Json<SayReq>) -> Json<serde_json::Value> {
    let text = req.text.trim().to_string();
    if text.is_empty() || text.len() > 400 {
        return Json(serde_json::json!({ "line": "…", "fallback": true }));
    }
    let npc = if req.npc.is_empty() {
        app.mission.lock().unwrap().as_ref().map(|m| m.fixer.clone()).unwrap_or_default()
    } else {
        req.npc
    };
    let (line, fallback) = llm::npc_reply(&app, &npc, &text).await;
    Json(serde_json::json!({ "npc": npc, "line": line, "fallback": fallback }))
}

async fn status(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "llm": app.orch.available().await }))
}

async fn stream(
    State(app): State<Arc<App>>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = app.tx.subscribe();
    let s = BroadcastStream::new(rx)
        .filter_map(|msg| msg.ok().map(|m| Ok(SseEvent::default().data(m))));
    Sse::new(s).keep_alive(KeepAlive::default())
}

/// Basic Auth gate, active only when AVALON_TOKEN is set. User: cobb.
async fn auth_gate(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let Ok(token) = std::env::var("AVALON_TOKEN") else {
        return next.run(req).await;
    };
    if token.is_empty() {
        return next.run(req).await;
    }
    use base64::Engine;
    let expected = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("cobb:{token}"))
    );
    let supplied = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if supplied == Some(expected.as_str()) {
        return next.run(req).await;
    }
    axum::response::Response::builder()
        .status(axum::http::StatusCode::UNAUTHORIZED)
        .header("WWW-Authenticate", "Basic realm=\"the depot\"")
        .body("Form 7C: credentials required, in duplicate.".into())
        .unwrap()
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
        "avalond: content — {} terrains, {} storylets, {} cargoes, {} npcs",
        content.terrains.len(),
        content.storylets.len(),
        content.cargoes.len(),
        content.npcs.len()
    );

    let (tx, _) = broadcast::channel(256);
    let orch = Arc::new(Orchestrator::new(LlmConfig {
        banned_phrases: content.banned_phrases.clone(),
        ..Default::default()
    }));
    let app = Arc::new(App {
        content,
        orch,
        mission: Mutex::new(None),
        chat: Mutex::new(Vec::new()),
        db: Mutex::new(conn),
        tx,
    });
    println!(
        "avalond: llm at {} ({})",
        app.orch.cfg.host,
        if app.orch.available().await { "reachable" } else { "unreachable — fallbacks only" }
    );

    let dist = PathBuf::from("web/dist");
    let static_svc = ServeDir::new(&dist).fallback(ServeFile::new(dist.join("index.html")));

    let router = Router::new()
        .route("/api/view", get(get_view))
        .route("/api/new", post(new_mission))
        .route("/api/abandon", post(abandon))
        .route("/api/choose", post(choose))
        .route("/api/say", post(say))
        .route("/api/status", get(status))
        .route("/api/stream", get(stream))
        .fallback_service(static_svc)
        .layer(axum::middleware::from_fn(auth_gate))
        .with_state(app);

    let bind = bind_addr();
    let listener = tokio::net::TcpListener::bind(&bind).await.expect("bind");
    println!(
        "avalond: listening on http://{bind} (auth: {})",
        if std::env::var("AVALON_TOKEN").map(|t| !t.is_empty()).unwrap_or(false) { "on" } else { "off" }
    );
    axum::serve(listener, router).await.expect("serve");
}
