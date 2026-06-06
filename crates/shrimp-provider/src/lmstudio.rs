use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    BoxStream, ChatMessage, ChatRequest, ChatResponse, ModelInfo, ModelProfile, Provider,
    ProviderError, StreamChunk,
};

#[derive(Serialize)]
struct LmRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct LmResponse {
    choices: Vec<LmChoice>,
    usage: Option<LmUsage>,
}

#[derive(Deserialize)]
struct LmChoice {
    message: LmMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct LmMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct LmUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

#[derive(Deserialize)]
struct LmModels {
    data: Vec<LmModel>,
}

#[derive(Deserialize)]
struct LmModel {
    id: String,
}

pub struct LmStudioProvider {
    client: reqwest::Client,
    base_url: String,
    profile: ModelProfile,
}

impl LmStudioProvider {
    pub fn new(base_url: String, model: String) -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            profile: ModelProfile::infer(&model),
            base_url,
        }
    }
}

#[async_trait]
impl Provider for LmStudioProvider {
    fn name(&self) -> &str {
        &self.profile.name
    }

    fn model_profile(&self) -> &ModelProfile {
        &self.profile
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let body = LmRequest {
            model: self.profile.name.clone(),
            messages: req.messages,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
        };

        let resp = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Api(format!("{status}: {text}")));
        }

        let parsed: LmResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;

        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| ProviderError::Api("empty choices array".to_string()))?;

        let content = choice.message.content.unwrap_or_default();
        let finish_reason = choice.finish_reason;
        let (prompt_tokens, completion_tokens) = parsed
            .usage
            .map(|u| (u.prompt_tokens, u.completion_tokens))
            .unwrap_or((0, 0));

        Ok(ChatResponse {
            content,
            tool_calls: vec![],
            prompt_tokens,
            completion_tokens,
            finish_reason,
        })
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamChunk, ProviderError>>, ProviderError> {
        let response = self.chat(req).await;

        let stream = futures_util::stream::once(async move {
            response.map(|r| StreamChunk {
                content_delta: r.content,
                done: true,
                prompt_tokens: r.prompt_tokens,
                completion_tokens: r.completion_tokens,
            })
        });

        Ok(Box::pin(stream))
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let resp = self
            .client
            .get(format!("{}/v1/models", self.base_url))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Api(format!("{status}: {text}")));
        }

        let parsed: LmModels = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;

        Ok(parsed
            .data
            .into_iter()
            .map(|m| ModelInfo {
                name: m.id,
                size_bytes: None,
            })
            .collect())
    }
}
