# Avalon

A single-player smuggling RPG for the browser. A Bedford TK, a deterministic
world, a local Qwen3-8B that renders but never decides — and a corn exchange
where your sins catch up with you, in triplicate.

1930s-flavoured England after a quiet collapse: guilds, militias, toll
warlords, and a bureaucracy that survived the apocalypse intact. You are
Cobb. You haul cargo, some of it licensed.

## The game

- **The Run** (core loop): take a contract at the Wychford depot, drive 3–5
  storylet beats (checkpoints, breakdowns, shakedowns, strangers), deliver,
  close the ledger. Choices seed flags; flags become *facts*; facts become
  NPC memories and gossip.
- **The Inquiry** (second mode): push your luck far enough and a Summons
  appears under the wiper. Witnesses testify from their real memories —
  including gossip that mutated in transit. Challenge testimony against the
  canonical record; contradictions are detected by pure data comparison.
  Win on the record, buy the verdict, or take the fine.
- **The oracle**: principal NPCs answer free text in character (local LLM),
  remember conversations, reflect on you between sessions, and reword
  rumours in their own voice. All of it validated; none of it canonical
  until the sim accepts it. If the model is down, authored lines ship
  instead — the game never blocks.

## Architecture (RFC-AVL-001)

| Crate | Role |
|---|---|
| `avalon-sim` | Pure deterministic core. No IO/tokio/f64. Integer pence/ml/ticks, carried ChaCha8 RNG. `(WorldState, Command, &ContentDb) -> (WorldState, Vec<Event>)`. Owns runs, memories, facts, gossip distortion, the Inquiry. |
| `avalon-content` | TOML content model + loader. Storylets (gates → prose → choices → effects), settlements, cargoes, NPC cards. Boot fails on invalid content. |
| `avalon-orchestrator` | LLM lanes. GPU = `Semaphore(1)`; interactive lane (6s budget, pre-empts) vs idle lane. Lint: think-tag strip, sentence cap, banned phrases. |
| `avalond` | The daemon: axum + SSE on `:4747`, SQLite save (WAL, append-only migrations), dialogue path, idle job worker (reflections, gossip rewording, embeddings, digests). |
| `web` | Thin Solid SPA on `:5173`. Renders state, posts intents, holds zero rules. |

**Invariants (do not break):**
- Canon changes only in `avalon-sim`. LLM output is cosmetic or a *proposal*
  (recorded-oracle Commands: `RecordChat`, `AddReflection`, `RewriteMemory`).
- Recorded-oracle contract (from townsfolk): validated LLM text is stored
  verbatim and replayed, never re-inferred.
- `scene()` never touches the RNG; choices validate against the derived scene.
- Migrations are append-only.

## Run it

```bash
cd llm && docker compose up -d   # avalon-qwen on :11436 (qwen3:8b + nomic-embed)
cargo run -p avalond             # daemon on :4747
cd web && npm run dev            # SPA on :5173
```

Open http://localhost:5173. Number keys choose; Space continues; type at
Arthur in town. The save is `saves/dev.db` (override `AVALON_SAVE`); delete
it for a fresh world. Real days away drift the economy and cool suspicion.
Without the LLM container everything still works — plainer.

Env knobs: `AVALON_OLLAMA` (default `http://127.0.0.1:11436`), `AVALON_MODEL`
(default `qwen3:8b`), `AVALON_CONTENT` (default `content/`), `AVALON_SAVE`.

## Tinkering map

- **Add a storylet**: drop a `[[storylet]]` in `content/storylets/*.toml`
  (gates/tests/effects validated at boot). One beat per template per run.
- **Add an NPC**: `content/npcs/*.toml` (persona feeds the LLM; barks are
  the fallback pool). Wire them into the social physics in
  `avalon-sim/src/memory.rs` (`flag_witness`) to make them see things.
- **New charges**: `chargeable()` in `memory.rs`.
- **Economy**: `content/world.toml` (cargo pay, settlement bonuses) and the
  constants atop `avalon-sim/src/engine.rs`.
- **Voice**: `banned_phrases` in `world.toml`; the bible in
  `avalond/src/llm.rs`.
- **Debug**: `GET /api/debug/world` (full state incl. memories/facts),
  `GET /api/status` (LLM + job queue), `sqlite3 saves/dev.db`.

API: `GET /api/view`, `POST /api/choose {idx}`, `POST /api/say {npc, text}`,
`GET /api/stream` (SSE), `GET /api/status`, `GET /api/debug/world`.
