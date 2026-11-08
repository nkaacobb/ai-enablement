//! The AI facade (`core/client.js`): resolves the settings and a key into an adapter call, streams the reply, and
//! falls back to a second provider when the first cannot be reached — only before any text or reasoning has arrived,
//! so an answer is never spliced from two models. [`HttpModel`] is the [`ChatModel`] a session talks to.

use crate::adapters::{Adapter, AdapterConfig, ChatCall, ModelInfo};
use crate::chat::{ChatModel, ChatRequest, ChatResult, EventKind, StreamEvent};
use crate::providers::{provider, Provider};
use crate::settings::Settings;
use crate::transport::{AiError, CancelToken, ErrorCode, HttpClient};
use std::sync::Arc;

/// The failures the fallback provider answers for: the first one could not be reached.
pub const FALLBACK_CODES: [ErrorCode; 3] = [ErrorCode::Network, ErrorCode::BadEndpoint, ErrorCode::Timeout];

/// A provider resolved from the settings: its preset, the adapter's configuration, the key and the adapter.
#[derive(Debug, Clone)]
pub struct Target {
    pub provider: &'static Provider,
    pub cfg: AdapterConfig,
    pub key: String,
    pub adapter: Adapter,
}

/// Settings + key → the target (`resolveTarget`). Fails when the provider cannot be used as configured: a cloud
/// provider without a key (unless the relay holds it), or no model where one is required (unless only listing models).
/// `cfg.relay_headers` is left empty (see [`Connection`]).
pub fn resolve_target(
    settings: &Settings,
    key_for: &dyn Fn(&str) -> String,
    provider_id: &str,
    for_listing: bool,
) -> Result<Target, AiError> {
    let p = provider(provider_id);
    let (base_url, model) = settings.profile_for(p.id);
    let key = key_for(p.id);
    let relay = settings.transport == "relay";
    if !relay && p.key_required && key.is_empty() {
        return Err(AiError::new(
            ErrorCode::Auth,
            format!("{} needs an API key. Open Settings > Model and paste one.", p.label),
        ));
    }
    if !for_listing && p.model_required && model.is_empty() {
        return Err(AiError::new(
            ErrorCode::MissingModel,
            format!("Choose a {} model in Settings > Model (try \"Load models\").", p.label),
        ));
    }
    let timeout_sec = if settings.timeout_sec > 0 { settings.timeout_sec as u64 } else { 120 };
    let cfg = AdapterConfig {
        provider: p.id.to_string(),
        protocol: p.protocol,
        label: p.label.to_string(),
        base_url,
        model,
        chat_path: p.chat_path.to_string(),
        models_path: p.models_path.to_string(),
        alt_models_path: p.alt_models_path.to_string(),
        token_field: p.token_field.to_string(),
        sends_temperature: p.sends_temperature,
        reasoning_off: p.reasoning_off_value(),
        reasoning: settings.reasoning.clone(),
        timeout_ms: timeout_sec * 1000,
        relay_url: if relay { settings.relay_url.clone() } else { String::new() },
        relay_headers: Vec::new(),
    };
    let adapter = if relay { Adapter::Relay } else { Adapter::for_protocol(p.protocol) };
    Ok(Target { provider: p, cfg, key, adapter })
}

/// What the client needs besides the settings: the HTTP client, the API key lookup (provider id → key, `""` for
/// none) and extra headers for relay requests (the app's session or CSRF header, say).
pub struct Connection<'a> {
    pub client: &'a dyn HttpClient,
    pub key_for: &'a dyn Fn(&str) -> String,
    pub relay_headers: Vec<(String, String)>,
}

impl<'a> Connection<'a> {
    pub fn new(client: &'a dyn HttpClient, key_for: &'a dyn Fn(&str) -> String) -> Self {
        Connection { client, key_for, relay_headers: Vec::new() }
    }

    /// [`resolve_target`] with this connection's key lookup and relay headers.
    pub fn target(&self, settings: &Settings, provider_id: &str, for_listing: bool) -> Result<Target, AiError> {
        let mut t = resolve_target(settings, self.key_for, provider_id, for_listing)?;
        t.cfg.relay_headers.clone_from(&self.relay_headers);
        Ok(t)
    }
}

/// Stream one reply (`streamChat`) with the settings' provider, reply limit and temperature. With tools in the
/// request the reply may end in tool calls instead of (or after) text. When the provider cannot be reached
/// ([`FALLBACK_CODES`]) before any text or reasoning arrived, and the settings name a fallback provider that can be
/// used, it answers instead: a notice event says so and the result has `fell_back`.
pub fn stream_chat(
    conn: &Connection<'_>,
    settings: &Settings,
    request: &ChatRequest,
    on_event: &mut dyn FnMut(StreamEvent),
    cancel: &CancelToken,
) -> Result<ChatResult, AiError> {
    let run = |t: &Target, forward: &mut dyn FnMut(StreamEvent)| {
        let call = ChatCall {
            cfg: &t.cfg,
            key: &t.key,
            request,
            max_tokens: Some(settings.max_output_tokens as f64),
            temperature: Some(settings.temperature),
        };
        t.adapter.stream(conn.client, &call, forward, cancel)
    };
    let label = |r: ChatResult, t: &Target, fell_back: bool| ChatResult {
        provider: t.provider.id.to_string(),
        label: t.provider.label.to_string(),
        model: t.cfg.model.clone(),
        fell_back,
        ..r
    };
    let primary = conn.target(settings, &settings.provider, false)?;
    let mut produced = false;
    let first = run(&primary, &mut |e: StreamEvent| {
        if matches!(e.kind, EventKind::Text | EventKind::Reasoning) && !e.text.is_empty() {
            produced = true;
        }
        on_event(e);
    });
    let e = match first {
        Ok(r) => return Ok(label(r, &primary, false)),
        Err(e) => e,
    };
    let fb_id = settings.fallback_provider.as_str();
    if fb_id.is_empty()
        || fb_id == primary.provider.id
        || produced
        || cancel.is_cancelled()
        || !FALLBACK_CODES.contains(&e.code)
    {
        return Err(e);
    }
    let Ok(fb) = conn.target(settings, fb_id, false) else { return Err(e) };
    on_event(StreamEvent::new(
        EventKind::Notice,
        format!("{} is not available — answering with {} instead.", primary.provider.label, fb.provider.label),
    ));
    let r = run(&fb, on_event)?;
    Ok(label(r, &fb, true))
}

/// Ask a provider which models it offers (`listModels`): the settings' provider unless `provider_id` names another;
/// `timeout_ms` replaces the settings' timeout.
pub fn list_models(
    conn: &Connection<'_>,
    settings: &Settings,
    provider_id: Option<&str>,
    timeout_ms: Option<u64>,
    cancel: &CancelToken,
) -> Result<Vec<ModelInfo>, AiError> {
    let id = provider_id.filter(|p| !p.is_empty()).unwrap_or(&settings.provider);
    let mut t = conn.target(settings, id, true)?;
    if let Some(ms) = timeout_ms.filter(|ms| *ms > 0) {
        t.cfg.timeout_ms = ms;
    }
    t.adapter.list_models(conn.client, &t.cfg, &t.key, cancel)
}

/// The API key of a provider (`""` for none).
pub type KeyLookup = Box<dyn Fn(&str) -> String + Send>;
/// Extra headers for relay requests, asked for each request.
pub type RelayHeaders = Box<dyn Fn() -> Vec<(String, String)> + Send>;

/// The configured provider over HTTP: a [`ChatModel`] for the session.
pub struct HttpModel {
    client: Arc<dyn HttpClient>,
    key_for: KeyLookup,
    relay_headers: Option<RelayHeaders>,
}

impl HttpModel {
    /// A model that sends through `client`, with the API keys from `key_for` (provider id → key, `""` for none).
    pub fn new(client: Arc<dyn HttpClient>, key_for: impl Fn(&str) -> String + Send + 'static) -> HttpModel {
        HttpModel { client, key_for: Box::new(key_for), relay_headers: None }
    }

    /// Add headers to every relay request (asked for each request).
    pub fn with_relay_headers(mut self, headers: impl Fn() -> Vec<(String, String)> + Send + 'static) -> HttpModel {
        self.relay_headers = Some(Box::new(headers));
        self
    }

    fn connection(&self) -> Connection<'_> {
        Connection {
            client: self.client.as_ref(),
            key_for: &*self.key_for,
            relay_headers: self.relay_headers.as_ref().map(|h| h()).unwrap_or_default(),
        }
    }

    /// The models a provider offers ([`list_models`]).
    pub fn list_models(
        &self,
        settings: &Settings,
        provider_id: Option<&str>,
        timeout_ms: Option<u64>,
        cancel: &CancelToken,
    ) -> Result<Vec<ModelInfo>, AiError> {
        list_models(&self.connection(), settings, provider_id, timeout_ms, cancel)
    }
}

impl std::fmt::Debug for HttpModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpModel").field("relay_headers", &self.relay_headers.is_some()).finish_non_exhaustive()
    }
}

impl ChatModel for HttpModel {
    fn chat(
        &mut self,
        request: &ChatRequest,
        settings: &Settings,
        on_event: &mut dyn FnMut(StreamEvent),
        cancel: &CancelToken,
    ) -> Result<ChatResult, AiError> {
        stream_chat(&self.connection(), settings, request, on_event, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::testing::{Reply, Scripted};
    use crate::chat::{RequestMessage, Role};
    use crate::json::Value;
    use crate::transport::HttpFailure;

    fn settings(json: &str) -> Settings {
        Settings::from_json(&Value::parse(json).unwrap())
    }

    fn question() -> ChatRequest {
        ChatRequest {
            messages: vec![RequestMessage { role: Role::User, content: "Hi".into(), images: vec![] }],
            ..Default::default()
        }
    }

    const FALLBACK: &str =
        r#"{"provider":"lmstudio","fallbackProvider":"ollama","profiles":{"ollama":{"model":"llama3"}}}"#;

    fn chat(
        model: &mut HttpModel,
        s: &Settings,
        cancel: &CancelToken,
    ) -> (Vec<StreamEvent>, Result<ChatResult, AiError>) {
        let mut events = Vec::new();
        let r = model.chat(&question(), s, &mut |e| events.push(e), cancel);
        (events, r)
    }

    #[test]
    fn falls_back_when_the_first_cannot_be_reached() {
        let client = Arc::new(Scripted::new(vec![
            Reply::fail(HttpFailure::Network("connection refused".into())),
            Reply::sse(&["data: {\"choices\":[{\"delta\":{\"content\":\"From Ollama.\"}}]}\n\n"]),
        ]));
        let mut model = HttpModel::new(client.clone(), |_| String::new());
        let (events, r) = chat(&mut model, &settings(FALLBACK), &CancelToken::new());
        let r = r.unwrap();
        assert_eq!(
            (r.provider.as_str(), r.label.as_str(), r.model.as_str(), r.fell_back),
            ("ollama", "Ollama (local)", "llama3", true)
        );
        assert_eq!(
            events[0],
            StreamEvent::new(
                EventKind::Notice,
                "LM Studio (local) is not available — answering with Ollama (local) instead."
            )
        );
        assert_eq!(events[1].text, "From Ollama.");
        let sent = client.requests();
        assert_eq!(sent[1].url, "http://127.0.0.1:11434/v1/chat/completions");
    }

    #[test]
    fn no_fallback_after_text_or_for_other_errors() {
        // Text arrived, then the stream broke: the error stands.
        let client = Arc::new(Scripted::new(vec![Reply::sse(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"Par\"}}]}\n\n",
            "data: {\"error\":\"Model crashed\"}\n\n",
        ])]));
        let mut model = HttpModel::new(client, |_| String::new());
        let (events, r) = chat(&mut model, &settings(FALLBACK), &CancelToken::new());
        assert_eq!(r.unwrap_err().code, ErrorCode::MissingModel);
        assert_eq!(events.len(), 1);
        // A refusal is not "cannot be reached".
        let client = Arc::new(Scripted::new(vec![Reply::json(401, r#"{"error":{"message":"no"}}"#)]));
        let mut model = HttpModel::new(client.clone(), |_| String::new());
        assert_eq!(chat(&mut model, &settings(FALLBACK), &CancelToken::new()).1.unwrap_err().code, ErrorCode::Auth);
        assert_eq!(client.requests().len(), 1);
        // Cancelled: no second request.
        let client = Arc::new(Scripted::new(vec![Reply::fail(HttpFailure::Network("x".into()))]));
        let mut model = HttpModel::new(client.clone(), |_| String::new());
        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(chat(&mut model, &settings(FALLBACK), &cancel).1.unwrap_err().code, ErrorCode::Cancelled);
        assert_eq!(client.requests().len(), 0);
    }

    #[test]
    fn unusable_fallback_keeps_the_first_error() {
        // OpenAI as the fallback, without a key: the network error of LM Studio stands.
        let client = Arc::new(Scripted::new(vec![Reply::fail(HttpFailure::Timeout)]));
        let mut model = HttpModel::new(client, |_| String::new());
        let s = settings(
            r#"{"provider":"lmstudio","fallbackProvider":"openai","profiles":{"openai":{"model":"gpt-5-mini"}}}"#,
        );
        let (events, r) = chat(&mut model, &s, &CancelToken::new());
        let e = r.unwrap_err();
        assert_eq!(e.code, ErrorCode::Timeout);
        assert_eq!(e.message, "No response for 120 s. The model took too long. Loading a model can be slow the first time; try again, or raise the timeout in Settings.");
        assert!(events.is_empty());
    }

    #[test]
    fn relay_headers_and_listing() {
        let client = Arc::new(Scripted::new(vec![
            Reply::json(200, r#"{"ok":true,"models":[{"id":"m1"}]}"#),
            Reply::json(200, r#"{"data":[{"id":"gpt-5-mini"}]}"#),
        ]));
        let model =
            HttpModel::new(client.clone(), |id| if id == "openai" { "sk-key-0001".into() } else { String::new() })
                .with_relay_headers(|| vec![("X-CSRF".into(), "t".into())]);
        let relay = settings(r#"{"provider":"openai","transport":"relay","relayUrl":"https://app.test/relay"}"#);
        let m = model.list_models(&relay, None, Some(5000), &CancelToken::new()).unwrap();
        assert_eq!(m[0].id, "m1");
        let direct = settings(r#"{"provider":"lmstudio"}"#);
        let m = model.list_models(&direct, Some("openai"), None, &CancelToken::new()).unwrap();
        assert_eq!(m[0].id, "gpt-5-mini");
        let sent = client.requests();
        assert_eq!(sent[0].timeout_ms, 5000);
        assert!(sent[0].headers.contains(&("X-Requested-With".into(), "ai-agent-drawer".into())));
        assert!(sent[0].headers.contains(&("X-CSRF".into(), "t".into())));
        assert_eq!(Value::parse(sent[0].body.as_deref().unwrap()).unwrap().at("action").as_str(), Some("models"));
        assert_eq!(sent[1].url, "https://api.openai.com/v1/models");
        assert!(sent[1].headers.contains(&("Authorization".into(), "Bearer sk-key-0001".into())));
    }

    #[test]
    fn listing_does_not_need_a_model() {
        let s = settings(r#"{"provider":"openai"}"#);
        let key = |_: &str| "sk-key-0001".to_string();
        assert!(resolve_target(&s, &key, "openai", true).is_ok());
        assert_eq!(resolve_target(&s, &key, "openai", false).unwrap_err().code, ErrorCode::MissingModel);
        let relay = settings(r#"{"provider":"anthropic","transport":"relay","relayUrl":"https://a.test/r"}"#);
        let t = resolve_target(&relay, &|_| String::new(), "anthropic", true).unwrap();
        assert_eq!((t.adapter, t.cfg.relay_url.as_str()), (Adapter::Relay, "https://a.test/r"));
    }
}
