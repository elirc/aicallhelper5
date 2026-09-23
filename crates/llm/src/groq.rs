//! Groq, OpenAI-compatible chat completions ("fastest" preset, spec §7).
//! Template for any other OpenAI-compatible provider.

use async_trait::async_trait;
use callcore_contract::config::MAX_TOKENS;
use callcore_contract::ports::{
    AnswerProvider, HeaderValue, PreparedRequest, PromptParts, ProviderFailure,
    ProviderFailureKind, StreamOutcome,
};
use callcore_contract::{Finish, Secret};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::failure::{fail, snippet};
use crate::http::{self, Emitter, Flow, ProviderMeta, StreamHandler, Throttle};
use crate::sse::SseEvent;

pub const ID: &str = "groq";
pub const DISPLAY_NAME: &str = "Groq GPT-OSS 120B (fastest)";
pub const KEY_ID: &str = "groq";
pub const MODEL: &str = "openai/gpt-oss-120b";

fn model_unavailable(status: u16, body: &str) -> bool {
    if status == 404 {
        return true;
    }
    if status != 400 {
        return false;
    }
    let lower = body.to_ascii_lowercase();
    lower.contains("model_decommissioned")
        || lower.contains("model_not_found")
        || lower.contains("does not exist")
}

pub(crate) const META: ProviderMeta = ProviderMeta {
    name: "Groq",
    model: MODEL,
    alternative: "Claude",
    model_unavailable,
};

pub struct GroqProvider {
    client: reqwest::Client,
    origin: String,
    throttle: Throttle,
}

impl GroqProvider {
    pub fn new(client: reqwest::Client, origin: &str) -> Self {
        Self {
            client,
            origin: origin.trim_end_matches('/').to_owned(),
            throttle: Throttle::default(),
        }
    }
}

/// The exact §7 request body. Never includes `reasoning_format`.
pub fn request_body(prompt: &PromptParts) -> Value {
    let system = format!("{}\n\n{}", prompt.cached_prefix, prompt.style_suffix);
    json!({
        "model": MODEL,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": prompt.user_message}
        ],
        "max_completion_tokens": MAX_TOKENS,
        "temperature": 0.7,
        "reasoning_effort": "low",
        "include_reasoning": false,
        "stream": true
    })
}

#[async_trait]
impl AnswerProvider for GroqProvider {
    fn id(&self) -> &'static str {
        ID
    }
    fn display_name(&self) -> &'static str {
        DISPLAY_NAME
    }
    fn key_id(&self) -> &'static str {
        KEY_ID
    }
    fn model(&self) -> &'static str {
        MODEL
    }

    fn build_request(&self, prompt: &PromptParts, key: &Secret) -> PreparedRequest {
        let body = serde_json::to_vec(&request_body(prompt)).unwrap_or_default();
        PreparedRequest {
            url: format!("{}/openai/v1/chat/completions", self.origin),
            headers: vec![
                (
                    "authorization".into(),
                    HeaderValue::Secret(format!("Bearer {}", key.expose())),
                ),
                (
                    "content-type".into(),
                    HeaderValue::Plain("application/json".into()),
                ),
            ],
            body: body.into(),
        }
    }

    async fn stream(
        &self,
        req: &PreparedRequest,
        deltas: mpsc::UnboundedSender<String>,
    ) -> Result<StreamOutcome, ProviderFailure> {
        let handler = Handler {
            done: false,
            finish_reason: None,
            secrets: http::secrets_of(req),
        };
        http::run_stream(&self.client, &META, req, deltas, handler).await
    }

    fn prewarm(&self) {
        http::prewarm(&self.client, &self.origin, &self.throttle);
    }
}

struct Handler {
    /// `[DONE]` seen: finish after the current network chunk.
    done: bool,
    finish_reason: Option<String>,
    secrets: Vec<String>,
}

impl StreamHandler for Handler {
    fn on_event(&mut self, ev: &SseEvent, out: &mut Emitter) -> Result<Flow, ProviderFailure> {
        let data = ev.data.trim();
        if data == "[DONE]" {
            self.done = true;
            return Ok(Flow::Continue);
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            tracing::debug!(len = data.len(), "groq: ignoring non-JSON SSE data");
            return Ok(Flow::Continue);
        };
        if let Some(err) = v.get("error") {
            let msg = snippet(&json!({ "error": err }).to_string(), &self.secrets);
            let detail = if msg.is_empty() {
                String::new()
            } else {
                format!(": {msg}")
            };
            return Err(fail(
                ProviderFailureKind::ProviderError,
                format!("Groq reported an error mid-answer{detail}"),
            ));
        }
        if let Some(choices) = v.get("choices").and_then(Value::as_array) {
            for c in choices {
                if let Some(t) = c.pointer("/delta/content").and_then(Value::as_str) {
                    out.emit(t)?;
                }
                if let Some(r) = c.get("finish_reason").and_then(Value::as_str) {
                    self.finish_reason = Some(r.to_owned());
                }
            }
        }
        Ok(Flow::Continue)
    }

    fn stop_after_chunk(&self) -> bool {
        self.done
    }

    fn terminal(&self) -> Option<Finish> {
        match self.finish_reason.as_deref() {
            Some("length") => Some(Finish::Truncated),
            Some("content_filter") => Some(Finish::Refused),
            Some(_) => Some(Finish::Complete),
            None if self.done => Some(Finish::Complete),
            None => None,
        }
    }
}
