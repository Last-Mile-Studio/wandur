//! The model providers (the C# `OpenAiCompatibleProvider`, `LmStudioNativeProvider` and
//! `AgentProviderRegistry`). Both send stateless requests, ask for no tools, and accept only
//! one complete decision that passes [`decision_codec::parse_decision`].
//!
//! | Provider | Path the settings add | Discovery | Inference |
//! | --- | --- | --- | --- |
//! | LM Studio (native API) | `/api/v1` | `models` (chat models only) | `chat` |
//! | OpenAI-compatible | `/v1` | `models` | `chat/completions` |
//!
//! Calls block; the runner and the settings make them on threads of their own.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Value, json};

use super::decision_codec::{self, AgentDecision, AgentRequest};
use super::profile::{self, AgentProfile, LMSTUDIO_NATIVE, OPENAI_COMPATIBLE};
use super::{AgentError, CancelToken, agent_http};

/// A way of asking a model for a decision.
pub trait AgentModelProvider: Send + Sync {
    /// The key profiles store (`openai-compatible`, `lmstudio-native`).
    fn key(&self) -> &str;
    fn decide(
        &self,
        request: &AgentRequest,
        api_key: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<AgentDecision, AgentError>;
    /// The models the server offers, sorted, without duplicates.
    fn list_models(
        &self,
        profile: &AgentProfile,
        api_key: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<Vec<String>, AgentError>;
}

/// The providers by key.
#[derive(Clone, Default)]
pub struct ProviderRegistry {
    providers: Vec<Arc<dyn AgentModelProvider>>,
}

impl std::fmt::Debug for ProviderRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.providers.iter().map(|p| p.key())).finish()
    }
}

impl ProviderRegistry {
    pub fn new(providers: Vec<Arc<dyn AgentModelProvider>>) -> Self {
        Self { providers }
    }

    /// The two HTTP providers.
    pub fn standard() -> Self {
        Self::new(vec![
            Arc::new(OpenAiCompatibleProvider),
            Arc::new(LmStudioNativeProvider),
        ])
    }

    pub fn resolve(&self, key: &str) -> Result<Arc<dyn AgentModelProvider>, AgentError> {
        self.providers
            .iter()
            .find(|p| p.key() == key)
            .cloned()
            .ok_or(AgentError::Invalid("Unsupported agent integration type."))
    }
}

/// A model id from a list: one line of 1 to 256 characters.
fn model_id(value: Option<&Value>) -> Result<String, AgentError> {
    let id = value.and_then(Value::as_str).ok_or(AgentError::InvalidResponse)?;
    if id.trim().is_empty() || super::len16(id) > 256 || id.chars().any(char::is_control) {
        return Err(AgentError::InvalidResponse);
    }
    Ok(id.to_string())
}

fn list<'a>(root: &'a Value, field: &str) -> Result<&'a Vec<Value>, AgentError> {
    let items = root
        .get(field)
        .and_then(Value::as_array)
        .ok_or(AgentError::InvalidResponse)?;
    if items.len() > 1024 {
        return Err(AgentError::InvalidResponse);
    }
    Ok(items)
}

/// Chat Completions (`/v1/chat/completions`): the model's text selects a fixed action.
#[derive(Clone, Copy, Debug, Default)]
pub struct OpenAiCompatibleProvider;

impl AgentModelProvider for OpenAiCompatibleProvider {
    fn key(&self) -> &str {
        OPENAI_COMPATIBLE
    }

    fn decide(
        &self,
        request: &AgentRequest,
        api_key: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<AgentDecision, AgentError> {
        let built = decision_codec::build(request)?;
        let mut payload = json!({
            "model": request.profile.model,
            "messages": [
                { "role": "system", "content": built.system },
                { "role": "user", "content": built.user },
            ],
            "max_tokens": request.profile.max_output_tokens,
            "temperature": 0,
            "stream": false,
        });
        if request.profile.json_mode {
            payload["response_format"] = json!({ "type": "json_object" });
        }
        let response = agent_http::send(&request.profile, "chat/completions", Some(&payload), api_key, cancel)?;
        let choices = response
            .get("choices")
            .and_then(Value::as_array)
            .ok_or(AgentError::InvalidResponse)?;
        let [choice] = choices.as_slice() else {
            return Err(AgentError::InvalidResponse);
        };
        if choice.get("finish_reason").and_then(Value::as_str) != Some("stop") {
            return Err(AgentError::InvalidResponse);
        }
        let message = choice.get("message").ok_or(AgentError::InvalidResponse)?;
        let tools_called = match message.get("tool_calls") {
            None | Some(Value::Null) => false,
            Some(Value::Array(calls)) => !calls.is_empty(),
            Some(_) => true,
        };
        if tools_called || message.get("function_call").is_some_and(|f| !f.is_null()) {
            return Err(AgentError::InvalidResponse);
        }
        let content = message
            .get("content")
            .and_then(Value::as_str)
            .ok_or(AgentError::InvalidResponse)?;
        decision_codec::parse_decision(content, &built.catalog)
    }

    fn list_models(
        &self,
        profile: &AgentProfile,
        api_key: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<Vec<String>, AgentError> {
        profile::validate(profile, false)?;
        let response = agent_http::send(profile, "models", None, api_key, cancel)?;
        let mut models = BTreeSet::new();
        for model in list(&response, "data")? {
            models.insert(model_id(model.get("id"))?);
        }
        Ok(models.into_iter().collect())
    }
}

/// LM Studio's native API (`/api/v1/chat`): stateless (`store: false`), no integrations.
#[derive(Clone, Copy, Debug, Default)]
pub struct LmStudioNativeProvider;

impl AgentModelProvider for LmStudioNativeProvider {
    fn key(&self) -> &str {
        LMSTUDIO_NATIVE
    }

    fn decide(
        &self,
        request: &AgentRequest,
        api_key: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<AgentDecision, AgentError> {
        let built = decision_codec::build(request)?;
        let payload = json!({
            "model": request.profile.model,
            "system_prompt": built.system,
            "input": built.user,
            "max_output_tokens": request.profile.max_output_tokens,
            "temperature": 0,
            "stream": false,
            "store": false,
            "integrations": [],
        });
        let response = agent_http::send(&request.profile, "chat", Some(&payload), api_key, cancel)?;
        let output = response
            .get("output")
            .and_then(Value::as_array)
            .ok_or(AgentError::InvalidResponse)?;
        if output.len() > 32 {
            return Err(AgentError::InvalidResponse);
        }
        let mut content = None;
        for item in output {
            match item.get("type").and_then(Value::as_str) {
                Some("reasoning") => {}
                Some("message") if content.is_none() => {
                    content = Some(
                        item.get("content")
                            .and_then(Value::as_str)
                            .ok_or(AgentError::InvalidResponse)?,
                    );
                }
                _ => return Err(AgentError::InvalidResponse),
            }
        }
        // The native API documents no finish reason: one complete, valid decision is required.
        decision_codec::parse_decision(content.ok_or(AgentError::InvalidResponse)?, &built.catalog)
    }

    fn list_models(
        &self,
        profile: &AgentProfile,
        api_key: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<Vec<String>, AgentError> {
        profile::validate(profile, false)?;
        let response = agent_http::send(profile, "models", None, api_key, cancel)?;
        let mut models = BTreeSet::new();
        for model in list(&response, "models")? {
            if model.get("type").and_then(Value::as_str) != Some("llm") {
                if model.get("type").is_none_or(|t| !t.is_string()) {
                    return Err(AgentError::InvalidResponse);
                }
                continue;
            }
            models.insert(model_id(model.get("key"))?);
        }
        Ok(models.into_iter().collect())
    }
}
