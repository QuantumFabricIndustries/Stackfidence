//! An optional real LLM agent, behind the `llm` feature flag.
//!
//! When `default-features = false` (the default), the framework runs purely
//! with mock agents and zero network dependencies. Enable `llm` to get an
//! agent that calls an OpenAI-compatible chat completions endpoint.
//!
//! Every call is recorded into the run record (layer 10) so the run is
//! replayable: the `Replayer` can feed the recorded responses back without
//! hitting the network.

use crate::agent::{AgentContext, AgentOutput};
use crate::error::{AgentError, AgentResult};
use crate::id::AgentId;
use crate::record::ModelCall;
use crate::value::Value;
use serde::{Deserialize, Serialize};

/// Configuration for the LLM agent.
pub struct LlmConfig {
    pub agent_id: AgentId,
    pub name: String,
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
}

/// A simple LLM agent that sends the input string as the user message and
/// returns the assistant's reply as `Value::Str`.
pub struct LlmAgent {
    cfg: LlmConfig,
    client: reqwest::blocking::Client,
}

impl LlmAgent {
    pub fn new(cfg: LlmConfig) -> Self {
        Self {
            cfg,
            client: reqwest::blocking::Client::new(),
        }
    }

    fn key_for(&self, input: &Value) -> String {
        // Deterministic key: hash of (model, input).
        let mut s = format!("{}|{}", self.cfg.model, input.to_json().to_string());
        // simple FNV-1a
        let mut hash: u64 = 0xcbf29ce484222325;
        for b in s.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        let _ = &mut s;
        format!("llm-{:x}", hash)
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
}

#[derive(Serialize, Deserialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessageOwned,
}

#[derive(Deserialize)]
struct ChatMessageOwned {
    content: String,
}

/// Token usage from the API response. OpenAI-compatible endpoints return:
/// `{"usage": {"prompt_tokens": N, "completion_tokens": M, "total_tokens": T}}`
#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

impl Usage {
    fn total(&self) -> u64 {
        if self.total_tokens > 0 {
            self.total_tokens
        } else {
            self.prompt_tokens + self.completion_tokens
        }
    }
}

impl crate::agent::Agent for LlmAgent {
    fn id(&self) -> AgentId {
        self.cfg.agent_id
    }
    fn name(&self) -> &str {
        &self.cfg.name
    }
    fn run(&self, ctx: &AgentContext) -> AgentResult<AgentOutput> {
        let user_content = match &ctx.input {
            Value::Str(s) => s.clone(),
            other => other.to_json().to_string(),
        };
        let key = self.key_for(&ctx.input);

        // If we have a recorded response for this key (replay), use it.
        if let Some(recorded) = ctx.record.lock().response_for(&key).cloned() {
            let reply = recorded
                .get("reply")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            return Ok(AgentOutput::done(Value::str(reply)));
        }

        // Otherwise call the endpoint.
        let req = ChatRequest {
            model: &self.cfg.model,
            messages: vec![ChatMessage {
                role: "user",
                content: user_content.clone(),
            }],
        };
        let resp = self
            .client
            .post(&self.cfg.endpoint)
            .bearer_auth(&self.cfg.api_key)
            .json(&req)
            .send()
            .map_err(|e| AgentError::Other(format!("llm request failed: {}", e)))?;
        let chat: ChatResponse = resp
            .json()
            .map_err(|e| AgentError::Other(format!("llm parse failed: {}", e)))?;
        let reply = chat
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();

        // Parse real token usage from the API response (layer 7 integration).
        let tokens = chat.usage.as_ref().map(|u| u.total()).unwrap_or(0);
        if tokens > 0 {
            ctx.spend(crate::budget::ResourceKind::Tokens, tokens)?;
        }

        // Record the call for replay (layer 10), with real token counts.
        ctx.record_model_call(ModelCall {
            span: ctx.span,
            caller: self.cfg.name.clone(),
            key: key.clone(),
            request: serde_json::json!({"input": user_content}),
            response: serde_json::json!({"reply": reply.clone(), "tokens": tokens}),
            tokens,
        });

        Ok(AgentOutput::done(Value::str(reply)))
    }
}
