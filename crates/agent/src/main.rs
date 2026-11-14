//! rogeha-agent: the AI assistant for Rogeha Reader, as a separate process.
//!
//! The reader starts this program only when the user opens the AI panel, so none of the AI
//! code (HTTP, TLS, JSON, the model loop) is loaded during normal reading.
//!
//! Bring your own provider:
//! * **Anthropic** (default): the Claude Messages API, streamed, with tool use.
//! * **OpenAI-compatible**: any `/chat/completions` endpoint with function calling: OpenAI,
//!   OpenRouter, or a local server such as Ollama, LM Studio or llama.cpp.
//!
//! The assistant never sees the whole document up front. It reads pages, searches and acts
//! through tools, and the *reader* runs every tool, so this process has no access to files.
//!
//! Protocol: one JSON object per line on stdin (from the reader) and stdout (to the reader).
//!   -> {"type":"hello"}                          <- {"type":"ready", ...settings}
//!   -> {"type":"configure", provider, model, base_url, effort, api_key?}   <- ready
//!   -> {"type":"ask","text":"...","context":"..."}
//!        <- {"type":"delta","text":"..."}          streamed answer text
//!        <- {"type":"tool_call","id","name","input"}   -> {"type":"tool_result","id","content","is_error"}
//!        <- {"type":"done"}  or  {"type":"error","text":"..."}
//!   -> {"type":"reset"}                          forget the conversation
//!
//! API keys: `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` from the environment if set, otherwise the
//! key saved through the reader, which is kept in the operating system's credential store
//! (Windows Credential Manager, macOS Keychain, Secret Service), never in a plain file.

use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;

use serde_json::{Value, json};

mod tools;

const SERVICE: &str = "Rogeha Reader";
const ANTHROPIC_VERSION: &str = "2026-10-21";
const DEFAULT_MODEL: &str = "claude-opus-5-5";
/// Models that support server-side refusal fallbacks (`fallbacks: "default"`).
const FALLBACK_MODELS: [&str; 5] = ["claude-fable-5-2", "claude-fable-5-1", "claude-opus-5-5", "claude-opus-5", "claude-sonnet-5-5"];
/// A turn may chain this many model calls (each tool round is one).
const MAX_ROUNDS: usize = 40;

const SYSTEM: &str = "You are the assistant built into Rogeha Reader, a PDF and EPUB reader. \
The user has a document open and talks to you in a side panel.\n\n\
Reading: you do not see the document until you read it. Use get_document_info for an overview, \
get_pages to read page text and search to find where something is mentioned. Read what you need \
before answering; for a summary of a long document, read it in batches. If a page has no text it \
may be a scan: say so and suggest \"Recognize text (OCR)\" in the Pages menu.\n\n\
Citations: after each statement taken from the document, cite the page as [p. 12] (or [p. 3-5]). \
The reader turns these into links. Only cite pages you have actually read. If the document does \
not answer the question, say so rather than guessing.\n\n\
Actions: when asked to change the document (rotate, delete, comment, highlight, sign, fill in a \
form), use the tools and then say briefly what you did. Changes are made in the open document and \
are not written to the file until the user saves. Deleting pages asks the user to confirm. If a \
request is unclear or risky, ask before acting.\n\n\
Style: answer in the language the user writes in. Be concise and plain. The panel shows plain \
text: no Markdown headings, tables or bold; short paragraphs and simple dashes for lists are fine.";

#[derive(Clone)]
struct Config {
    /// "anthropic" or "openai" (any OpenAI-compatible endpoint).
    provider: String,
    model: String,
    base_url: String,
    effort: String,
}

impl Default for Config {
    fn default() -> Self {
        Self { provider: "anthropic".into(), model: DEFAULT_MODEL.into(), base_url: String::new(), effort: "medium".into() }
    }
}

fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ROGEHA_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    };
    base.unwrap_or_else(std::env::temp_dir).join(SERVICE)
}

impl Config {
    fn load() -> Self {
        let mut c = Self::default();
        let Ok(text) = std::fs::read_to_string(data_dir().join("assistant.json")) else { return c };
        let Ok(v) = serde_json::from_str::<Value>(&text) else { return c };
        let get = |k: &str, d: &str| v[k].as_str().filter(|s| !s.is_empty()).unwrap_or(d).to_owned();
        c.provider = get("provider", &c.provider);
        c.model = get("model", &c.model);
        c.base_url = get("base_url", "");
        c.effort = get("effort", &c.effort);
        c
    }

    fn save(&self) {
        let dir = data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let v = json!({ "provider": self.provider, "model": self.model, "base_url": self.base_url, "effort": self.effort });
        let _ = std::fs::write(dir.join("assistant.json"), v.to_string());
    }

    fn key_name(&self) -> &'static str {
        if self.provider == "anthropic" { "anthropic-api-key" } else { "openai-compatible-api-key" }
    }

    /// The API key: environment first, then the OS credential store.
    fn api_key(&self) -> Option<String> {
        let env = if self.provider == "anthropic" { "ANTHROPIC_API_KEY" } else { "OPENAI_API_KEY" };
        if let Some(key) = std::env::var(env).ok().filter(|k| !k.trim().is_empty()) {
            return Some(key);
        }
        keyring::Entry::new(SERVICE, self.key_name()).ok()?.get_password().ok().filter(|k| !k.is_empty())
    }

    fn store_key(&self, key: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(SERVICE, self.key_name()).map_err(|e| e.to_string())?;
        if key.is_empty() {
            let _ = entry.delete_credential();
            Ok(())
        } else {
            entry.set_password(key).map_err(|e| format!("Could not save the key in the system's credential store: {e}"))
        }
    }

    fn anthropic_url(&self) -> String {
        // The base can be overridden for testing or a proxy.
        let base = std::env::var("ROGEHA_ANTHROPIC_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".into());
        format!("{}/v1/messages", base.trim_end_matches('/'))
    }

    fn openai_url(&self) -> String {
        let base = if self.base_url.trim().is_empty() { "https://api.openai.com/v1" } else { self.base_url.trim() };
        format!("{}/chat/completions", base.trim_end_matches('/'))
    }

    fn ready(&self, note: &str) -> Value {
        let has_key = self.api_key().is_some();
        let needs_key = self.provider == "anthropic" || self.base_url.trim().is_empty();
        let status = if !note.is_empty() {
            note.to_owned()
        } else if needs_key && !has_key {
            "Add an API key in the settings below to start.".to_owned()
        } else {
            format!("Ready: {}", self.model)
        };
        json!({
            "type": "ready", "text": status,
            "provider": self.provider, "model": self.model, "base_url": self.base_url, "effort": self.effort,
            "has_key": has_key,
        })
    }
}

struct Session {
    config: Config,
    http: ureq::Agent,
    /// Conversation so far, in the current provider's message format.
    messages: Vec<Value>,
    input: io::Lines<BufReader<io::Stdin>>,
    out: io::Stdout,
}

impl Session {
    fn send(&mut self, v: Value) {
        let mut out = self.out.lock();
        let _ = writeln!(out, "{v}");
        let _ = out.flush();
    }

    /// Ask the reader to run a tool and wait for its answer.
    fn run_tool(&mut self, id: &str, name: &str, input: &Value) -> (String, bool) {
        if let Err(problem) = tools::validate(name, input) {
            return (problem, true);
        }
        self.send(json!({ "type": "tool_call", "id": id, "name": name, "input": input }));
        for line in self.input.by_ref() {
            let Ok(line) = line else { break };
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v["type"] == "tool_result" && v["id"] == id {
                return (v["content"].as_str().unwrap_or_default().to_owned(), v["is_error"].as_bool().unwrap_or(false));
            }
        }
        // The reader went away.
        std::process::exit(0);
    }

    fn ask(&mut self, text: &str, context: &str) -> Result<(), String> {
        let needs_key = self.config.provider == "anthropic" || self.config.base_url.trim().is_empty();
        let key = self.config.api_key();
        if needs_key && key.is_none() {
            return Err("No API key yet. Open the assistant's settings and add your key.".into());
        }
        let prompt = if context.is_empty() { text.to_owned() } else { format!("{context}

{text}") };
        let before = self.messages.len();
        self.messages.push(json!({ "role": "user", "content": prompt }));
        let result = if self.config.provider == "anthropic" {
            self.ask_anthropic(key.as_deref().unwrap_or_default())
        } else {
            self.ask_openai(key.as_deref())
        };
        if result.is_err() {
            // Keep the history valid for the next question: drop the turn that failed.
            self.messages.truncate(before);
        }
        result
    }

    // ------------------------------------------------------------------ Claude

    fn ask_anthropic(&mut self, key: &str) -> Result<(), String> {
        for _ in 0..MAX_ROUNDS {
            let mut body = json!({
                "model": self.config.model,
                "max_tokens": 64000,
                "stream": true,
                "system": [{ "type": "text", "text": SYSTEM }],
                "tools": tools::anthropic(),
                "messages": self.messages,
                // Thinking is adaptive by default on current models; effort sets its depth.
                "output_config": { "effort": self.config.effort },
                // Cache the growing conversation (page text the model has read) between rounds.
                "cache_control": { "type": "ephemeral" },
            });
            let mut request = self
                .http
                .post(&self.config.anthropic_url())
                .header("content-type", "application/json")
                .header("x-api-key", key)
                .header("anthropic-version", ANTHROPIC_VERSION);
            if FALLBACK_MODELS.contains(&self.config.model.as_str()) {
                // If a safety classifier declines, let the API retry on its default fallback model.
                body["fallbacks"] = json!("default");
                request = request.header("anthropic-beta", "server-side-fallback-2026-07-01");
            }
            let mut response = request.send_json(&body).map_err(|e| format!("Could not reach the Claude API: {e}"))?;
            let status = response.status().as_u16();
            if status != 200 {
                let detail = response.body_mut().read_to_string().unwrap_or_default();
                return Err(api_error("Claude API", status, &detail));
            }

            // Rebuild the message from the event stream; text is forwarded as it arrives.
            let mut blocks: Vec<Value> = Vec::new();
            let mut partial_json: Vec<String> = Vec::new();
            let mut stop_reason = String::new();
            let reader = BufReader::new(response.body_mut().as_reader());
            for line in reader.lines() {
                let line = line.map_err(|e| format!("The connection to the Claude API was interrupted: {e}"))?;
                let Some(data) = line.strip_prefix("data:") else { continue };
                let Ok(event) = serde_json::from_str::<Value>(data.trim()) else { continue };
                let index = event["index"].as_u64().unwrap_or(0) as usize;
                match event["type"].as_str() {
                    Some("content_block_start") => {
                        while blocks.len() <= index {
                            blocks.push(Value::Null);
                            partial_json.push(String::new());
                        }
                        blocks[index] = event["content_block"].clone();
                    }
                    Some("content_block_delta") if index < blocks.len() => {
                        let delta = &event["delta"];
                        let append = |block: &mut Value, field: &str, more: &str| {
                            let joined = format!("{}{more}", block[field].as_str().unwrap_or_default());
                            block[field] = Value::String(joined);
                        };
                        match delta["type"].as_str() {
                            Some("text_delta") => {
                                let more = delta["text"].as_str().unwrap_or_default();
                                append(&mut blocks[index], "text", more);
                                self.send(json!({ "type": "delta", "text": more }));
                            }
                            Some("input_json_delta") => partial_json[index].push_str(delta["partial_json"].as_str().unwrap_or_default()),
                            Some("thinking_delta") => append(&mut blocks[index], "thinking", delta["thinking"].as_str().unwrap_or_default()),
                            Some("signature_delta") => blocks[index]["signature"] = delta["signature"].clone(),
                            _ => {}
                        }
                    }
                    Some("content_block_stop") if index < blocks.len() => {
                        if blocks[index]["type"] == "tool_use" {
                            // Tool input streams as text; it may be cut short or malformed.
                            let raw = std::mem::take(&mut partial_json[index]);
                            match serde_json::from_str::<Value>(if raw.trim().is_empty() { "{}" } else { &raw }) {
                                Ok(input) if input.is_object() => blocks[index]["input"] = input,
                                _ => {
                                    blocks[index]["input"] = json!({});
                                    blocks[index]["rr_invalid"] = json!(true);
                                }
                            }
                        }
                    }
                    Some("message_delta") => {
                        if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                            stop_reason = reason.to_owned();
                        }
                    }
                    Some("error") => {
                        return Err(format!("The Claude API reported a problem: {}", event["error"]["message"].as_str().unwrap_or("unknown error")));
                    }
                    _ => {}
                }
            }
            blocks.retain(|b| !b.is_null());

            // Check how the turn ended before acting on its content.
            let invalid: Vec<String> = blocks.iter().filter(|b| b["rr_invalid"] == true).filter_map(|b| b["id"].as_str().map(str::to_owned)).collect();
            for block in &mut blocks {
                if let Some(map) = block.as_object_mut() {
                    map.remove("rr_invalid");
                }
            }
            let calls: Vec<(String, String, Value)> = blocks
                .iter()
                .filter(|b| b["type"] == "tool_use")
                .map(|b| (b["id"].as_str().unwrap_or_default().to_owned(), b["name"].as_str().unwrap_or_default().to_owned(), b["input"].clone()))
                .collect();
            // Everything the model produced goes back unchanged next round (thinking included).
            self.messages.push(json!({ "role": "assistant", "content": blocks }));

            match stop_reason.as_str() {
                "tool_use" => {
                    self.send(json!({ "type": "delta", "text": "\n" }));
                    // All results go back together in one user message.
                    let mut results = Vec::new();
                    for (id, name, input) in calls {
                        let (content, is_error) = if invalid.contains(&id) {
                            ("INVALID_JSON: the tool input was not valid JSON; send it again.".to_owned(), true)
                        } else {
                            self.run_tool(&id, &name, &input)
                        };
                        results.push(json!({ "type": "tool_result", "tool_use_id": id, "content": content, "is_error": is_error }));
                    }
                    self.messages.push(json!({ "role": "user", "content": results }));
                }
                // A server-side pause: send the conversation back to let the model continue.
                "pause_turn" => {}
                "refusal" => {
                    self.send(json!({ "type": "delta", "text": "\nThe model declined to answer this request." }));
                    return Ok(());
                }
                "max_tokens" => {
                    self.send(json!({ "type": "delta", "text": "\n(The answer was cut off because it reached the length limit.)" }));
                    return Ok(());
                }
                _ => return Ok(()),
            }
        }
        Err("The assistant took too many steps and was stopped.".into())
    }

    // ------------------------------------------------------- OpenAI-compatible

    fn ask_openai(&mut self, key: Option<&str>) -> Result<(), String> {
        for _ in 0..MAX_ROUNDS {
            let mut messages = vec![json!({ "role": "system", "content": SYSTEM })];
            messages.extend(self.messages.iter().cloned());
            let body = json!({ "model": self.config.model, "messages": messages, "tools": tools::openai() });
            let mut request = self.http.post(&self.config.openai_url()).header("content-type", "application/json");
            if let Some(key) = key {
                request = request.header("authorization", &format!("Bearer {key}"));
            }
            let mut response = request.send_json(&body).map_err(|e| format!("Could not reach {}: {e}", self.config.openai_url()))?;
            let status = response.status().as_u16();
            let text = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
            if status != 200 {
                return Err(api_error("The provider", status, &text));
            }
            let reply: Value = serde_json::from_str(&text).map_err(|_| "The provider sent an answer that could not be read.".to_owned())?;
            let message = reply["choices"][0]["message"].clone();
            if !message.is_object() {
                return Err("The provider sent an empty answer.".into());
            }
            if let Some(content) = message["content"].as_str().filter(|c| !c.is_empty()) {
                self.send(json!({ "type": "delta", "text": content }));
            }
            let calls = message["tool_calls"].as_array().cloned().unwrap_or_default();
            self.messages.push(message);
            if calls.is_empty() {
                return Ok(());
            }
            self.send(json!({ "type": "delta", "text": "\n" }));
            for call in calls {
                let id = call["id"].as_str().unwrap_or_default().to_owned();
                let name = call["function"]["name"].as_str().unwrap_or_default().to_owned();
                // Arguments arrive as a JSON string; parse it, never match on the raw text.
                let parsed = serde_json::from_str::<Value>(call["function"]["arguments"].as_str().unwrap_or("{}"));
                let (content, _) = match parsed {
                    Ok(input) if input.is_object() => self.run_tool(&id, &name, &input),
                    _ => ("The tool arguments were not valid JSON; send them again.".to_owned(), true),
                };
                self.messages.push(json!({ "role": "tool", "tool_call_id": id, "content": content }));
            }
        }
        Err("The assistant took too many steps and was stopped.".into())
    }
}

/// A message for people from an HTTP error status and the provider's error body.
fn api_error(who: &str, status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| body.chars().take(200).collect());
    match status {
        401 | 403 => format!("{who} did not accept the API key. Check it in the assistant's settings. ({detail})"),
        404 => format!("{who} does not know this model or address. Check the settings. ({detail})"),
        429 => format!("{who} is rate limiting or the account is out of credit. Try again shortly. ({detail})"),
        500..=599 => format!("{who} is having trouble right now (status {status}). Try again shortly."),
        _ => format!("{who} refused the request (status {status}): {detail}"),
    }
}

fn main() {
    let http: ureq::Agent = ureq::Agent::config_builder()
        // Errors are read from the response body, and long answers must not time out.
        .http_status_as_error(false)
        .timeout_global(None)
        .build()
        .into();
    let mut session = Session {
        config: Config::load(),
        http,
        messages: Vec::new(),
        input: BufReader::new(io::stdin()).lines(),
        out: io::stdout(),
    };

    while let Some(Ok(line)) = session.input.next() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            session.send(json!({ "type": "error", "text": "Bad request" }));
            continue;
        };
        match request["type"].as_str() {
            Some("hello") => {
                let ready = session.config.ready("");
                session.send(ready);
            }
            Some("configure") => {
                let text = |k: &str| request[k].as_str().map(str::trim).map(str::to_owned);
                let mut config = session.config.clone();
                if let Some(provider) = text("provider").filter(|p| p == "anthropic" || p == "openai") {
                    config.provider = provider;
                }
                if let Some(model) = text("model").filter(|m| !m.is_empty()) {
                    config.model = model;
                }
                if let Some(base_url) = text("base_url") {
                    config.base_url = base_url;
                }
                if let Some(effort) = text("effort").filter(|e| ["low", "medium", "high", "xhigh", "max"].contains(&e.as_str())) {
                    config.effort = effort;
                }
                let mut note = String::new();
                if let Some(key) = text("api_key") {
                    if let Err(e) = config.store_key(&key) {
                        note = e;
                    }
                }
                config.save();
                // A different provider or model starts a fresh conversation.
                session.messages.clear();
                session.config = config;
                let ready = session.config.ready(&note);
                session.send(ready);
            }
            Some("reset") => {
                session.messages.clear();
                let ready = session.config.ready("");
                session.send(ready);
            }
            Some("ask") => {
                let text = request["text"].as_str().unwrap_or_default().to_owned();
                let context = request["context"].as_str().unwrap_or_default().to_owned();
                match session.ask(&text, &context) {
                    Ok(()) => session.send(json!({ "type": "done" })),
                    Err(e) => session.send(json!({ "type": "error", "text": e })),
                }
            }
            other => session.send(json!({ "type": "error", "text": format!("Unknown request type: {other:?}") })),
        }
    }
}
