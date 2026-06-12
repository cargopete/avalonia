//! avalon-orchestrator — three-lane LLM job queue, GPU gate, Ollama client,
//! validation pipeline (RFC-AVL-001 DR-5). Stub until Stage 3.
//!
//! Hardware notes from Stage-0 recon (RTX 2000 Ada Laptop, 8 GB):
//! - Two Ollamas may exist on this host (native :11434, thrushcombe :11435);
//!   one 8B model fits the card at a time — probe actual GPU residency, don't
//!   just trust our own semaphore.
//! - Cold model load is ~45 s; manage keep_alive by session lifecycle (long
//!   while a session is open, release at Ledger Close).
//! - Avalon gets its own pinned Ollama container; never share thrushcombe's.
