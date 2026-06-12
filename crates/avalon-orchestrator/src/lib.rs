//! avalon-orchestrator — the LLM lanes, GPU gate, Ollama client, and the
//! validation pipeline (RFC-AVL-001 DR-5; call catalog in the design doc).
//!
//! Rules, in order of importance:
//! - The sim is canon; everything out of here is cosmetic or a *proposal*.
//! - The game never blocks on the model: every call has a deterministic
//!   fallback and a hard timeout. Interactive lane <6s, idle lane minutes.
//! - Structured output XOR thinking (mutually exclusive in Ollama); all
//!   interactive calls run thinking OFF.
//! - One in-flight request: a single 8 GB card is a `Semaphore(1)`.
//!
//! Hardware notes (Stage-0 recon): RTX 2000 Ada 8 GB, ~40 tok/s warm; cold
//! load ~45 s, hence per-request keep_alive while a session is live.

use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

#[derive(Clone, Debug)]
pub struct LlmConfig {
    pub host: String,
    pub model: String,
    pub embed_model: String,
    /// Sentence cap for dialogue lines.
    pub max_sentences: usize,
    pub banned_phrases: Vec<String>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            host: std::env::var("AVALON_OLLAMA")
                .unwrap_or_else(|_| "http://127.0.0.1:11436".into()),
            model: std::env::var("AVALON_MODEL").unwrap_or_else(|_| "qwen3:8b".into()),
            embed_model: "nomic-embed-text".into(),
            max_sentences: 3,
            banned_phrases: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lane {
    /// Player is waiting: short timeout, pre-empts idle work.
    Interactive,
    /// Between-sessions work: long timeout, yields to interactive.
    Idle,
}

pub struct Orchestrator {
    pub cfg: LlmConfig,
    client: reqwest::Client,
    gate: Semaphore,
    last_interactive: Mutex<Instant>,
}

impl Orchestrator {
    pub fn new(cfg: LlmConfig) -> Self {
        Self {
            cfg,
            client: reqwest::Client::new(),
            gate: Semaphore::new(1),
            last_interactive: Mutex::new(Instant::now() - Duration::from_secs(3600)),
        }
    }

    /// Is the model server reachable at all? (Cheap; used for the UI badge.)
    pub async fn available(&self) -> bool {
        let url = format!("{}/api/tags", self.cfg.host);
        matches!(
            self.client.get(&url).timeout(Duration::from_millis(800)).send().await,
            Ok(r) if r.status().is_success()
        )
    }

    /// May the idle lane run right now? Yields for 8s after interactive work.
    pub fn idle_clear(&self) -> bool {
        self.last_interactive.lock().unwrap().elapsed() > Duration::from_secs(8)
    }

    /// One validated generation. Returns None on timeout/error/lint-fail —
    /// the caller ships its authored fallback. Retry budget: zero here; the
    /// catalog's one-repair-retry belongs to specific call sites that can
    /// strengthen the prompt.
    pub async fn say(&self, lane: Lane, system: &str, user: &str) -> Option<String> {
        let (timeout, think) = match lane {
            Lane::Interactive => (Duration::from_secs(6), false),
            Lane::Idle => (Duration::from_secs(120), false),
        };
        if lane == Lane::Interactive {
            *self.last_interactive.lock().unwrap() = Instant::now();
        }
        let _permit = match lane {
            Lane::Interactive => {
                // Don't queue behind a stuck idle job longer than the budget.
                tokio::time::timeout(timeout, self.gate.acquire()).await.ok()?.ok()?
            }
            Lane::Idle => self.gate.acquire().await.ok()?,
        };
        let body = json!({
            "model": self.cfg.model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
            "stream": false,
            "think": think,
            "keep_alive": "30m",
            "options": {
                // Qwen3 non-thinking sampling per the model card; presence
                // penalty per the quantized-model guidance (Q4 repetition).
                "temperature": 0.7,
                "top_p": 0.8,
                "top_k": 20,
                "min_p": 0.0,
                "presence_penalty": 1.5,
                "num_predict": 160,
            },
        });
        #[derive(Deserialize)]
        struct ChatResp {
            message: ChatMsg,
        }
        #[derive(Deserialize)]
        struct ChatMsg {
            content: String,
        }
        let url = format!("{}/api/chat", self.cfg.host);
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .timeout(timeout)
            .send()
            .await
            .ok()?
            .json::<ChatResp>()
            .await
            .ok()?;
        self.lint(&resp.message.content)
    }

    /// Validation stages 1+3 (schema is the JSON parse above; fact-check is
    /// the caller's, who knows the entities): strip think tags, cap length,
    /// reject banned phrases and obvious anachronisms. None = fallback.
    pub fn lint(&self, raw: &str) -> Option<String> {
        let mut text = raw.trim().to_string();
        // Belt-and-braces: Ollama think:false has historically leaked tags.
        if let Some(end) = text.find("</think>") {
            text = text[end + 8..].trim().to_string();
        }
        text = text.trim_matches('"').trim().to_string();
        if text.is_empty() || text.len() > 600 {
            return None;
        }
        let lower = text.to_lowercase();
        for banned in &self.cfg.banned_phrases {
            if lower.contains(&banned.to_lowercase()) {
                return None;
            }
        }
        // Sentence cap: keep the first N, drop the rest silently.
        let mut sentences = 0;
        let mut cut = text.len();
        for (i, ch) in text.char_indices() {
            if matches!(ch, '.' | '!' | '?') {
                sentences += 1;
                if sentences >= self.cfg.max_sentences {
                    cut = i + ch.len_utf8();
                    break;
                }
            }
        }
        text.truncate(cut);
        let text = text.trim().to_string();
        (!text.is_empty()).then_some(text)
    }

    /// Embedding for memory retrieval. None when the embedder is missing —
    /// retrieval degrades to recency+importance, which is fine.
    pub async fn embed(&self, text: &str) -> Option<Vec<f32>> {
        let url = format!("{}/api/embeddings", self.cfg.host);
        let body = json!({ "model": self.cfg.embed_model, "prompt": text });
        let resp: Value = self
            .client
            .post(&url)
            .json(&body)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        let arr = resp.get("embedding")?.as_array()?;
        Some(arr.iter().filter_map(|v| v.as_f64().map(|f| f as f32)).collect())
    }
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn orch() -> Orchestrator {
        Orchestrator::new(LlmConfig {
            banned_phrases: vec!["delve".into(), "I daresay".into()],
            ..Default::default()
        })
    }

    #[test]
    fn lint_strips_think_and_caps_sentences() {
        let o = orch();
        let raw = "<think>reasoning</think> One. Two! Three? Four.";
        assert_eq!(o.lint(raw).as_deref(), Some("One. Two! Three?"));
    }

    #[test]
    fn lint_rejects_banned_phrases() {
        let o = orch();
        assert_eq!(o.lint("Let us delve into the ledger."), None);
        assert_eq!(o.lint("I daresay the barley is fine."), None);
    }

    #[test]
    fn cosine_sane() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    }
}
