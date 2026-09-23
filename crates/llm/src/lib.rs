//! Answer providers. PUBLIC API PINNED (the shell wires exactly these).

use std::sync::Arc;

use callcore_contract::ports::{AnswerProvider, ProviderRegistry};
use callcore_contract::ProviderInfo;

pub mod anthropic;
mod failure;
pub mod groq;
mod http;
pub mod sse;

pub use anthropic::AnthropicProvider;
pub use failure::failure_to_app_error;
pub use groq::GroqProvider;

pub const ANTHROPIC_ORIGIN: &str = "https://api.anthropic.com";
pub const GROQ_ORIGIN: &str = "https://api.groq.com";

/// The ONE shared HTTP client (pooled, rustls, transport timeouts from
/// `callcore_contract::config`).
pub fn http_client() -> reqwest::Client {
    http::build_client()
}

/// Provider registry. Adding a provider = one module + one line in `new`.
pub struct Registry {
    _providers: Vec<Arc<dyn AnswerProvider>>,
}

impl Registry {
    /// Production origins.
    pub fn new(client: reqwest::Client) -> Self {
        Self::with_origins(client, ANTHROPIC_ORIGIN, GROQ_ORIGIN)
    }

    /// Test/bench seam: override each provider's origin (e.g. a loopback
    /// server "http://127.0.0.1:PORT").
    pub fn with_origins(
        client: reqwest::Client,
        anthropic_origin: &str,
        groq_origin: &str,
    ) -> Self {
        // The FIRST entry is the default provider.
        let _providers: Vec<Arc<dyn AnswerProvider>> = vec![
            Arc::new(AnthropicProvider::new(client.clone(), anthropic_origin)),
            Arc::new(GroqProvider::new(client, groq_origin)),
        ];
        Self { _providers }
    }

    /// `{id, displayName, keyId, model}` for the settings view.
    pub fn infos(&self) -> Vec<ProviderInfo> {
        self._providers
            .iter()
            .map(|p| ProviderInfo {
                id: p.id().to_owned(),
                display_name: p.display_name().to_owned(),
                key_id: p.key_id().to_owned(),
                model: p.model().to_owned(),
            })
            .collect()
    }
}

impl ProviderRegistry for Registry {
    fn get(&self, id: &str) -> Option<Arc<dyn AnswerProvider>> {
        self._providers.iter().find(|p| p.id() == id).cloned()
    }
    fn default_provider(&self) -> Arc<dyn AnswerProvider> {
        self._providers[0].clone()
    }
    fn all(&self) -> Vec<Arc<dyn AnswerProvider>> {
        self._providers.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_both_providers_with_anthropic_default() {
        let r = Registry::new(http_client());
        let infos = r.infos();
        assert_eq!(
            infos,
            vec![
                ProviderInfo {
                    id: "anthropic".into(),
                    display_name: "Claude Haiku 4.5 (recommended)".into(),
                    key_id: "anthropic".into(),
                    model: "claude-haiku-4-5".into(),
                },
                ProviderInfo {
                    id: "groq".into(),
                    display_name: "Groq GPT-OSS 120B (fastest)".into(),
                    key_id: "groq".into(),
                    model: "openai/gpt-oss-120b".into(),
                },
            ]
        );
        assert_eq!(r.default_provider().id(), "anthropic");
        assert_eq!(r.get("groq").map(|p| p.id()), Some("groq"));
        assert!(r.get("openai").is_none());
        assert_eq!(r.all().len(), 2);
    }
}
