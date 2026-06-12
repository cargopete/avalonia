# Avalon

Single-player smuggling RPG: a Bedford TK, a deterministic world, and a local
Qwen3-8B that renders but never decides. Design docs: the session-scope
recommendation, the LLM call architecture, and RFC-AVL-001 (topology).

## Layout

- `crates/avalon-sim` — pure deterministic core. No IO, integer time/money,
  carried seeded RNG. `(WorldState, Command) -> (WorldState, Vec<Event>)`.
- `crates/avalon-content` — TOML storylet/NPC loaders (Stage 2).
- `crates/avalon-orchestrator` — three-lane LLM queue, validation pipeline (Stage 3).
- `crates/avalond` — the daemon: axum API + SSE, SQLite save (WAL,
  `user_version` migrations), sessions, jobs.
- `web` — thin Solid SPA. Renders state, posts intents, holds zero rules.
- `content/`, `prompts/`, `tools/` — data, prompt templates, dev tooling.

## Run (dev)

```bash
cargo run -p avalond          # daemon on http://127.0.0.1:4747
cd web && npm run dev         # SPA on http://localhost:5173 (proxies /api)
```

Number keys pick choices; Space continues when there's only one way forward.
One Run = Town Phase → three road beats → Arrival → Ledger Close. The save
lives in `saves/dev.db` (override with `AVALON_SAVE=path`); delete it for a
fresh world. Real days away drift the economy at next boot (delta-time);
game days do not pass while you're gone.

API: `GET /api/view` (scene + ledger), `POST /api/choose {idx}`,
`GET /api/stream` (SSE narration).

## Invariants (do not break)

- Canon changes only in `avalon-sim`. LLM output is a proposal or cosmetics.
- `avalon-sim` stays pure and wasm32-clean: no tokio, no IO, no f64 in canon.
- Recorded-oracle contract: validated LLM text is logged as immutable events
  and replayed verbatim, never re-inferred.
- Migrations are append-only.
