//! rogeha-agent: the AI agent for Rogeha Reader, as a separate process.
//!
//! The reader starts this program only when the user opens the AI panel, so none of the
//! AI code (HTTP, TLS, JSON, model logic) is loaded during normal reading.
//!
//! Protocol: one JSON object per line on stdin (requests) and stdout (replies).
//!   -> {"type":"hello"}
//!   <- {"type":"ready","text":"...","providers":[...],"default_model":"..."}
//!   -> {"type":"ask","text":"...","page":3,"document":"C:/..."}
//!   <- {"type":"message","text":"..."}   or   {"type":"error","text":"..."}
//!
//! STATUS: milestone 7 is not implemented yet. This stub proves the lazy-loading design
//! and answers every request with a clear "not available yet" message.

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

/// Providers planned for milestone 7 (bring your own key / endpoint).
const PROVIDERS: [&str; 3] = ["anthropic", "openai-compatible", "local"];
/// Default Claude model; configurable in settings once the agent is implemented.
const DEFAULT_MODEL: &str = "claude-opus-5-5";

fn main() {
    let stdin = io::stdin();
    let mut out = io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(req) => handle(&req),
            Err(e) => json!({"type": "error", "text": format!("Bad request: {e}")}),
        };
        if writeln!(out, "{reply}").and_then(|_| out.flush()).is_err() {
            break;
        }
    }
}

fn handle(req: &Value) -> Value {
    match req["type"].as_str() {
        Some("hello") => json!({
            "type": "ready",
            "text": "Agent process started (preview: no AI provider is connected yet).",
            "providers": PROVIDERS,
            "default_model": DEFAULT_MODEL,
        }),
        Some("ask") => json!({
            "type": "message",
            "text": "The AI assistant isn't implemented yet. Planned: summaries, questions with \
                     page citations, translation and page actions, using your own Claude API key, \
                     an OpenAI-compatible endpoint or a local model.",
        }),
        other => json!({"type": "error", "text": format!("Unknown request type: {other:?}")}),
    }
}
