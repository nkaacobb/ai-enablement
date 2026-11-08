//! What the host allows (`spec/behaviour.md` §9, "A host may restrict providers"): which providers the agent may talk
//! to, and a gate every request passes (e.g. the user's consent before their data reaches a model). Both are enforced
//! by [`GuardedModel`], around any [`ChatModel`], before a byte leaves the app.

use crate::chat::{ChatModel, ChatRequest, ChatResult, StreamEvent};
use crate::providers::{is_local_url, is_loopback_url, provider, Provider, PROVIDERS};
use crate::settings::Settings;
use crate::transport::{AiError, CancelToken, ErrorCode};

/// Which providers the agent may use.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ProviderPolicy {
    /// Any provider the user configures.
    #[default]
    Any,
    /// Only a server on this computer (`localhost`, `127.x`, `::1`), and no relay.
    LoopbackOnly,
    /// Only this computer or the local network.
    LocalOnly,
    /// Only these provider ids (at any address).
    Providers(Vec<String>),
}

impl ProviderPolicy {
    /// Why this provider (at its configured address) is not allowed, or Ok.
    pub fn check(&self, settings: &Settings, provider_id: &str) -> Result<(), AiError> {
        let p = provider(provider_id);
        let (base, _) = settings.profile_for(p.id);
        let refuse = |why: String| Err(AiError::new(ErrorCode::Refused, why));
        match self {
            ProviderPolicy::Any => Ok(()),
            ProviderPolicy::LoopbackOnly | ProviderPolicy::LocalOnly if settings.transport == "relay" => {
                refuse("This application sends questions only to a model on this computer, not through a relay. Switch \"Send requests\" back to direct in Settings > Model.".into())
            }
            ProviderPolicy::LoopbackOnly if !is_loopback_url(&base) => refuse(format!(
                "This application allows only a model running on this computer. {} at {base} is not one: choose a local provider (LM Studio, Ollama) at 127.0.0.1 in Settings > Model.",
                p.label
            )),
            ProviderPolicy::LocalOnly if !is_local_url(&base) => refuse(format!(
                "This application allows only models on this computer or the local network. {} at {base} is not one.",
                p.label
            )),
            ProviderPolicy::Providers(ids) if !ids.iter().any(|i| i == p.id) => {
                refuse(format!("This application does not allow {}. Choose another provider in Settings > Model.", p.label))
            }
            _ => Ok(()),
        }
    }

    /// The providers the settings screen offers under this policy (an address the user changes is checked again
    /// before each request).
    pub fn providers(&self) -> Vec<&'static Provider> {
        PROVIDERS
            .iter()
            .filter(|p| match self {
                ProviderPolicy::Any => true,
                ProviderPolicy::LoopbackOnly => is_loopback_url(p.base_url),
                ProviderPolicy::LocalOnly => is_local_url(p.base_url),
                ProviderPolicy::Providers(ids) => ids.iter().any(|i| i == p.id),
            })
            .collect()
    }
}

/// A gate every request passes: Err(message) refuses it, and the drawer shows the message.
pub type BeforeSend = Box<dyn FnMut(&ChatRequest, &Settings) -> Result<(), String> + Send>;

/// A model behind the host's rules: the provider policy (the fallback provider too) and the `before_send` gate.
pub struct GuardedModel<M: ChatModel> {
    pub inner: M,
    pub policy: ProviderPolicy,
    pub before_send: Option<BeforeSend>,
}

impl<M: ChatModel> GuardedModel<M> {
    pub fn new(inner: M, policy: ProviderPolicy) -> Self {
        GuardedModel { inner, policy, before_send: None }
    }
}

impl<M: ChatModel> ChatModel for GuardedModel<M> {
    fn chat(
        &mut self,
        request: &ChatRequest,
        settings: &Settings,
        on_event: &mut dyn FnMut(StreamEvent),
        cancel: &CancelToken,
    ) -> Result<ChatResult, AiError> {
        self.policy.check(settings, &settings.provider)?;
        let mut effective = settings.clone();
        if !effective.fallback_provider.is_empty() && self.policy.check(settings, &effective.fallback_provider).is_err()
        {
            effective.fallback_provider.clear();
        }
        if let Some(gate) = self.before_send.as_mut() {
            gate(request, &effective).map_err(|why| AiError::new(ErrorCode::Refused, why))?;
        }
        self.inner.chat(request, &effective, on_event, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::Value;

    struct Echo(Vec<String>);
    impl ChatModel for Echo {
        fn chat(
            &mut self,
            _: &ChatRequest,
            s: &Settings,
            _: &mut dyn FnMut(StreamEvent),
            _: &CancelToken,
        ) -> Result<ChatResult, AiError> {
            self.0.push(format!("{}>{}", s.provider, s.fallback_provider));
            Ok(ChatResult::default())
        }
    }

    fn settings(json: &str) -> Settings {
        Settings::from_json(&Value::parse(json).unwrap())
    }

    #[test]
    fn loopback_only() {
        let p = ProviderPolicy::LoopbackOnly;
        assert!(p.check(&settings(r#"{"provider":"lmstudio"}"#), "lmstudio").is_ok());
        assert!(p.check(&settings(r#"{"provider":"openai"}"#), "openai").is_err());
        let lan = settings(r#"{"provider":"lmstudio","profiles":{"lmstudio":{"baseUrl":"http://192.168.1.9:1234"}}}"#);
        assert!(p.check(&lan, "lmstudio").is_err());
        assert!(ProviderPolicy::LocalOnly.check(&lan, "lmstudio").is_ok());
        assert!(p
            .check(&settings(r#"{"provider":"lmstudio","transport":"relay","relayUrl":"/r"}"#), "lmstudio")
            .is_err());
        assert_eq!(p.providers().iter().map(|x| x.id).collect::<Vec<_>>(), ["lmstudio", "ollama", "custom"]);
    }

    #[test]
    fn guarded_model() {
        let mut m = GuardedModel::new(Echo(vec![]), ProviderPolicy::LoopbackOnly);
        let cancel = CancelToken::new();
        let req = ChatRequest::default();
        let s = settings(r#"{"provider":"lmstudio","fallbackProvider":"openai"}"#);
        m.chat(&req, &s, &mut |_| {}, &cancel).unwrap();
        assert_eq!(m.inner.0, ["lmstudio>"], "a fallback the policy refuses is dropped");
        let e = m.chat(&req, &settings(r#"{"provider":"anthropic"}"#), &mut |_| {}, &cancel).unwrap_err();
        assert_eq!(e.code, ErrorCode::Refused);
        m.before_send = Some(Box::new(|_, _| Err("Consent is needed before patient text is sent.".into())));
        let e = m.chat(&req, &s, &mut |_| {}, &cancel).unwrap_err();
        assert_eq!(e.message, "Consent is needed before patient text is sent.");
        assert_eq!(m.inner.0.len(), 1, "nothing reached the model");
    }
}
