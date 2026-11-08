//! The provider catalog (`core/providers.js`): a provider is a preset for the editable settings (address, model,
//! key) plus the wire protocol it speaks. There is deliberately no model list: models come from "Load models" or are
//! typed.

use crate::json::Value;

/// The wire protocols (one adapter each, `crate::adapters`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    /// `POST {base}/v1/chat/completions` (LM Studio, Ollama, OpenAI, DeepSeek, OpenRouter, custom servers).
    OpenAiChat,
    /// `POST {base}/v1/messages`.
    AnthropicMessages,
    /// `POST {base}/v1beta/models/{model}:streamGenerateContent?alt=sse`.
    GeminiGenerate,
}

impl Protocol {
    pub fn id(self) -> &'static str {
        match self {
            Protocol::OpenAiChat => "openai-chat",
            Protocol::AnthropicMessages => "anthropic-messages",
            Protocol::GeminiGenerate => "gemini-generate",
        }
    }
}

/// Where a provider runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Local,
    Custom,
    Cloud,
}

/// One provider preset.
#[derive(Debug, Clone, PartialEq)]
pub struct Provider {
    pub id: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub kind: Kind,
    pub protocol: Protocol,
    pub base_url: &'static str,
    pub chat_path: &'static str,
    pub models_path: &'static str,
    pub alt_models_path: &'static str,
    /// The body field for the reply limit (`max_tokens`, or OpenAI's `max_completion_tokens`).
    pub token_field: &'static str,
    pub sends_temperature: bool,
    pub key_required: bool,
    pub key_optional: bool,
    pub model_required: bool,
    /// JSON merged into the body when reasoning is off (provider-specific), or `None`.
    pub reasoning_off: Option<&'static str>,
    pub placeholder: &'static str,
    pub key_env: &'static str,
}

impl Provider {
    /// The "reasoning off" switch as a JSON object, when the provider has one.
    pub fn reasoning_off_value(&self) -> Option<Value> {
        self.reasoning_off.and_then(|s| Value::parse(s).ok())
    }
}

const fn preset(
    id: &'static str,
    label: &'static str,
    kind: Kind,
    protocol: Protocol,
    base_url: &'static str,
) -> Provider {
    Provider {
        id,
        label,
        group: match kind {
            Kind::Cloud => "Cloud",
            _ => "On this computer",
        },
        kind,
        protocol,
        base_url,
        chat_path: "",
        models_path: "",
        alt_models_path: "",
        token_field: "max_tokens",
        sends_temperature: true,
        key_required: false,
        key_optional: false,
        model_required: true,
        reasoning_off: None,
        placeholder: "",
        key_env: "",
    }
}

/// Every provider, in the order the settings list them.
pub static PROVIDERS: &[Provider] = &[
    Provider {
        chat_path: "/v1/chat/completions",
        models_path: "/api/v1/models",
        alt_models_path: "/v1/models",
        key_optional: true,
        model_required: false,
        reasoning_off: Some(r#"{"reasoning_effort":"none"}"#),
        placeholder: "Leave empty to use whatever model is loaded",
        ..preset("lmstudio", "LM Studio (local)", Kind::Local, Protocol::OpenAiChat, "http://127.0.0.1:9000")
    },
    Provider {
        chat_path: "/v1/chat/completions",
        models_path: "/v1/models",
        key_optional: true,
        placeholder: "e.g. llama3.2 or qwen3:8b",
        reasoning_off: Some(r#"{"reasoning_effort":"none"}"#),
        ..preset("ollama", "Ollama (local)", Kind::Local, Protocol::OpenAiChat, "http://127.0.0.1:11434")
    },
    Provider {
        chat_path: "/v1/chat/completions",
        models_path: "/v1/models",
        key_optional: true,
        model_required: false,
        placeholder: "model id the server expects",
        ..preset(
            "custom",
            "Other OpenAI-compatible server",
            Kind::Custom,
            Protocol::OpenAiChat,
            "http://127.0.0.1:8080",
        )
    },
    Provider {
        chat_path: "/v1/chat/completions",
        models_path: "/v1/models",
        token_field: "max_completion_tokens",
        sends_temperature: false,
        key_required: true,
        key_env: "OPENAI_API_KEY",
        placeholder: "e.g. gpt-5-mini",
        ..preset("openai", "OpenAI", Kind::Cloud, Protocol::OpenAiChat, "https://api.openai.com")
    },
    Provider {
        models_path: "/v1/models?limit=1000",
        sends_temperature: false,
        key_required: true,
        key_env: "ANTHROPIC_API_KEY",
        placeholder: "e.g. claude-sonnet-5",
        ..preset(
            "anthropic",
            "Anthropic (Claude)",
            Kind::Cloud,
            Protocol::AnthropicMessages,
            "https://api.anthropic.com",
        )
    },
    Provider {
        models_path: "/v1beta/models?pageSize=1000",
        key_required: true,
        key_env: "GEMINI_API_KEY",
        placeholder: "e.g. gemini-2.5-flash",
        ..preset(
            "google",
            "Google Gemini",
            Kind::Cloud,
            Protocol::GeminiGenerate,
            "https://generativelanguage.googleapis.com",
        )
    },
    Provider {
        chat_path: "/chat/completions",
        models_path: "/models",
        key_required: true,
        key_env: "DEEPSEEK_API_KEY",
        placeholder: "e.g. deepseek-chat",
        reasoning_off: Some(r#"{"thinking":{"type":"disabled"}}"#),
        ..preset("deepseek", "DeepSeek", Kind::Cloud, Protocol::OpenAiChat, "https://api.deepseek.com")
    },
    Provider {
        chat_path: "/v1/chat/completions",
        models_path: "/v1/models",
        key_required: true,
        key_env: "OPENROUTER_API_KEY",
        placeholder: "e.g. anthropic/claude-sonnet-5",
        ..preset(
            "openrouter",
            "OpenRouter (many models)",
            Kind::Cloud,
            Protocol::OpenAiChat,
            "https://openrouter.ai/api",
        )
    },
];

/// The provider ids, in catalog order.
pub fn provider_ids() -> impl Iterator<Item = &'static str> {
    PROVIDERS.iter().map(|p| p.id)
}

/// The provider with this id; LM Studio for an unknown one (as the JS runtime does).
pub fn provider(id: &str) -> &'static Provider {
    PROVIDERS.iter().find(|p| p.id == id).unwrap_or(&PROVIDERS[0])
}

/// The host of an http(s) URL, lowercased, without brackets around an IPv6 address; `None` when it is not one.
pub fn url_host(url: &str) -> Option<String> {
    let rest = url.trim();
    let lower = rest.to_ascii_lowercase();
    let after = if lower.starts_with("http://") {
        &rest[7..]
    } else if lower.starts_with("https://") {
        &rest[8..]
    } else {
        return None;
    };
    let authority = after.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    let host = if let Some(stripped) = host_port.strip_prefix('[') {
        stripped.split(']').next().unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

/// True for loopback, private-network and `.local` hosts: the data stays on this machine or the LAN.
pub fn is_local_url(url: &str) -> bool {
    let Some(h) = url_host(url) else { return false };
    if h == "localhost" || h.ends_with(".localhost") || h.ends_with(".local") || h == "::1" || h == "0.0.0.0" {
        return true;
    }
    let Some([a, b, _, _]) = ipv4(&h) else { return false };
    a == 127 || a == 10 || (a == 192 && b == 168) || (a == 172 && (16..=31).contains(&b))
}

/// The four parts of a dotted IPv4 address, or `None`.
fn ipv4(host: &str) -> Option<[u32; 4]> {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut out = [0u32; 4];
    for (i, p) in parts.iter().enumerate() {
        if p.is_empty() || p.len() > 3 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        out[i] = p.parse().ok().filter(|n| *n <= 255)?;
    }
    Some(out)
}

/// True for loopback hosts only (`localhost`, `127.x.x.x`, `::1`): what a loopback-only provider policy allows.
pub fn is_loopback_url(url: &str) -> bool {
    let Some(h) = url_host(url) else { return false };
    h == "localhost" || h.ends_with(".localhost") || h == "::1" || ipv4(&h).is_some_and(|a| a[0] == 127)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_order_and_fallback() {
        assert_eq!(
            provider_ids().collect::<Vec<_>>(),
            ["lmstudio", "ollama", "custom", "openai", "anthropic", "google", "deepseek", "openrouter"]
        );
        assert_eq!(provider("nope").id, "lmstudio");
        assert_eq!(provider("openai").token_field, "max_completion_tokens");
        assert!(provider("deepseek").reasoning_off_value().is_some());
    }

    #[test]
    fn local_and_loopback() {
        assert!(is_local_url("http://127.0.0.1:9000"));
        assert!(is_local_url("http://[::1]:1234/v1"));
        assert!(is_local_url("http://192.168.1.5"));
        assert!(is_local_url("http://box.local:80"));
        assert!(!is_local_url("https://api.openai.com"));
        assert!(!is_local_url("ftp://127.0.0.1"));
        assert!(is_loopback_url("http://localhost:9000"));
        assert!(!is_loopback_url("http://192.168.1.5"));
        assert!(!is_loopback_url("http://user@evil.com@127.0.0.1.evil.com/"));
    }
}
