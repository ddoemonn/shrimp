use async_trait::async_trait;
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use thiserror::Error;

pub mod lmstudio;
pub mod ollama;
pub use lmstudio::LmStudioProvider;
pub use ollama::OllamaProvider;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProviderKind {
    #[default]
    Ollama,
    LmStudio,
}

impl ProviderKind {
    pub fn as_str(&self) -> &str {
        match self {
            ProviderKind::Ollama => "ollama",
            ProviderKind::LmStudio => "lmstudio",
        }
    }

    pub fn default_base_url(&self) -> &str {
        match self {
            ProviderKind::Ollama => "http://localhost:11434",
            ProviderKind::LmStudio => "http://localhost:1234",
        }
    }

    pub fn display_name(&self) -> &str {
        match self {
            ProviderKind::Ollama => "Ollama",
            ProviderKind::LmStudio => "LM Studio",
        }
    }

    pub fn resolve_base_url(&self, custom_url: Option<&str>) -> String {
        if let Some(url) = custom_url {
            let trimmed = url.trim();
            if !trimmed.is_empty() && trimmed != self.default_base_url() {
                return normalize_url(trimmed);
            }
        }

        match self {
            ProviderKind::Ollama => {
                if let Ok(env_val) = std::env::var("OLLAMA_HOST") {
                    if !env_val.trim().is_empty() {
                        return normalize_url(&env_val);
                    }
                }
            }
            ProviderKind::LmStudio => {
                if let Ok(env_val) = std::env::var("LM_STUDIO_HOST") {
                    if !env_val.trim().is_empty() {
                        return normalize_url(&env_val);
                    }
                }
            }
        }

        if let Some(url) = custom_url {
            let trimmed = url.trim();
            if !trimmed.is_empty() {
                return normalize_url(trimmed);
            }
        }
        self.default_base_url().to_string()
    }
}

pub fn normalize_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("http://{}", trimmed)
    }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub base_url: String,
    pub model: String,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            kind: ProviderKind::Ollama,
            base_url: "http://localhost:11434".to_string(),
            model: "qwen2.5-coder:7b".to_string(),
        }
    }
}

impl ProviderConfig {
    pub fn new(kind: ProviderKind, base_url: String, model: String) -> Self {
        Self {
            kind,
            base_url,
            model,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StreamChunk {
    pub content_delta: String,
    pub done: bool,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ModelProfile {
    pub name: String,
    pub context_window: u32,
    pub supports_tools: bool,
}

impl ModelProfile {
    pub fn infer(name: &str) -> Self {
        let lower = name.to_lowercase();
        let (context_window, supports_tools) =
            if lower.contains("qwen2.5-coder") || lower.contains("deepseek-coder") {
                (32768, true)
            } else if lower.contains("qwen") || lower.contains("deepseek") {
                (32768, false)
            } else if lower.contains("llama3") || lower.contains("llama-3") {
                (8192, false)
            } else if lower.contains("mistral") || lower.contains("mixtral") {
                (32768, false)
            } else if lower.contains("codellama") {
                (16384, false)
            } else if lower.contains("phi") {
                (4096, false)
            } else {
                (8192, false)
            };
        Self {
            name: name.to_string(),
            context_window,
            supports_tools,
        }
    }
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("api error: {0}")]
    Api(String),
    #[error("parse error: {0}")]
    Parse(String),
}

pub type BoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + Send + 'a>>;

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn model_profile(&self) -> &ModelProfile;
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError>;
    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamChunk, ProviderError>>, ProviderError>;
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError>;
}

pub enum AnyProvider {
    Ollama(OllamaProvider),
    LmStudio(LmStudioProvider),
}

impl AnyProvider {
    pub fn name(&self) -> &str {
        match self {
            AnyProvider::Ollama(p) => p.name(),
            AnyProvider::LmStudio(p) => p.name(),
        }
    }

    pub fn model_profile(&self) -> &ModelProfile {
        match self {
            AnyProvider::Ollama(p) => p.model_profile(),
            AnyProvider::LmStudio(p) => p.model_profile(),
        }
    }

    pub async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        match self {
            AnyProvider::Ollama(p) => p.chat(req).await,
            AnyProvider::LmStudio(p) => p.chat(req).await,
        }
    }

    pub async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamChunk, ProviderError>>, ProviderError> {
        match self {
            AnyProvider::Ollama(p) => p.chat_stream(req).await,
            AnyProvider::LmStudio(p) => p.chat_stream(req).await,
        }
    }

    pub async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        match self {
            AnyProvider::Ollama(p) => p.list_models().await,
            AnyProvider::LmStudio(p) => p.list_models().await,
        }
    }
}

pub fn create_provider(config: &ProviderConfig) -> Result<AnyProvider, ProviderError> {
    match config.kind {
        ProviderKind::Ollama => Ok(AnyProvider::Ollama(OllamaProvider::new(
            config.base_url.clone(),
            config.model.clone(),
        ))),
        ProviderKind::LmStudio => Ok(AnyProvider::LmStudio(LmStudioProvider::new(
            config.base_url.clone(),
            config.model.clone(),
        ))),
    }
}

pub async fn list_models(config: &ProviderConfig) -> Result<Vec<ModelInfo>, ProviderError> {
    match config.kind {
        ProviderKind::Ollama => {
            OllamaProvider::new(config.base_url.clone(), config.model.clone())
                .list_models()
                .await
        }
        ProviderKind::LmStudio => {
            LmStudioProvider::new(config.base_url.clone(), config.model.clone())
                .list_models()
                .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_url() {
        assert_eq!(normalize_url("hub:11434"), "http://hub:11434");
        assert_eq!(normalize_url("http://hub:11434"), "http://hub:11434");
        assert_eq!(normalize_url("https://hub:11434"), "https://hub:11434");
        assert_eq!(normalize_url("   127.0.0.1:1234  "), "http://127.0.0.1:1234");
        assert_eq!(normalize_url(""), "");
    }

    #[test]
    fn test_resolve_base_url() {
        // Setup environments
        std::env::remove_var("OLLAMA_HOST");
        std::env::remove_var("LM_STUDIO_HOST");

        // 1. If custom_url is explicitly set and different from default, use it.
        assert_eq!(
            ProviderKind::Ollama.resolve_base_url(Some("http://hub:11434")),
            "http://hub:11434"
        );
        assert_eq!(
            ProviderKind::LmStudio.resolve_base_url(Some("http://hub:1234")),
            "http://hub:1234"
        );

        // 2. If custom_url is the default (or none), and environment var is set, use the env var.
        std::env::set_var("OLLAMA_HOST", "my-ollama:11434");
        std::env::set_var("LM_STUDIO_HOST", "my-lmstudio:1234");

        assert_eq!(
            ProviderKind::Ollama.resolve_base_url(None),
            "http://my-ollama:11434"
        );
        assert_eq!(
            ProviderKind::Ollama.resolve_base_url(Some("http://localhost:11434")),
            "http://my-ollama:11434"
        );
        assert_eq!(
            ProviderKind::LmStudio.resolve_base_url(None),
            "http://my-lmstudio:1234"
        );
        assert_eq!(
            ProviderKind::LmStudio.resolve_base_url(Some("http://localhost:1234")),
            "http://my-lmstudio:1234"
        );

        // Cleanup
        std::env::remove_var("OLLAMA_HOST");
        std::env::remove_var("LM_STUDIO_HOST");

        // 3. Fallback to default
        assert_eq!(
            ProviderKind::Ollama.resolve_base_url(None),
            "http://localhost:11434"
        );
        assert_eq!(
            ProviderKind::LmStudio.resolve_base_url(None),
            "http://localhost:1234"
        );
    }
}

