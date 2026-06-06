use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use crate::{
    BoxStream, ChatMessage, ChatRequest, ChatResponse, ModelInfo, ModelProfile, Provider,
    ProviderError, StreamChunk,
};

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    options: OllamaOptions,
}

#[derive(Serialize)]
struct OllamaOptions {
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
}

#[derive(Deserialize)]
struct OllamaResponse {
    message: OllamaMessage,
    done: bool,
    prompt_eval_count: Option<u32>,
    eval_count: Option<u32>,
    done_reason: Option<String>,
}

#[derive(Deserialize)]
struct OllamaMessage {
    content: String,
}

#[derive(Deserialize)]
struct OllamaStreamChunk {
    message: Option<OllamaMessage>,
    done: bool,
    prompt_eval_count: Option<u32>,
    eval_count: Option<u32>,
}

#[derive(Deserialize)]
struct TagsResponse {
    models: Vec<OllamaModel>,
}

#[derive(Deserialize)]
struct OllamaModel {
    name: String,
    size: Option<u64>,
}

pub struct OllamaProvider {
    client: reqwest::Client,
    base_url: String,
    profile: ModelProfile,
}

impl OllamaProvider {
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
impl Provider for OllamaProvider {
    fn name(&self) -> &str {
        &self.profile.name
    }

    fn model_profile(&self) -> &ModelProfile {
        &self.profile
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let body = OllamaRequest {
            model: self.profile.name.clone(),
            messages: req.messages,
            stream: false,
            options: OllamaOptions {
                temperature: req.temperature,
                num_predict: req.max_tokens,
            },
        };

        let resp = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Api(format!("{status}: {text}")));
        }

        let parsed: OllamaResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;

        Ok(ChatResponse {
            content: parsed.message.content,
            tool_calls: vec![],
            prompt_tokens: parsed.prompt_eval_count.unwrap_or(0),
            completion_tokens: parsed.eval_count.unwrap_or(0),
            finish_reason: if parsed.done {
                parsed.done_reason
            } else {
                None
            },
        })
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamChunk, ProviderError>>, ProviderError> {
        let body = OllamaRequest {
            model: self.profile.name.clone(),
            messages: req.messages,
            stream: true,
            options: OllamaOptions {
                temperature: req.temperature,
                num_predict: req.max_tokens,
            },
        };

        let resp = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Api(format!("{status}: {text}")));
        }

        let byte_stream = resp.bytes_stream();

        let stream = futures_util::stream::unfold(
            (byte_stream, Vec::<u8>::new()),
            |(mut byte_stream, mut buf)| async move {
                loop {
                    if let Some(newline_pos) = buf.iter().position(|&b| b == b'\n') {
                        let line = buf.drain(..newline_pos + 1).collect::<Vec<_>>();
                        let trimmed = line.trim_ascii();
                        if trimmed.is_empty() {
                            continue;
                        }
                        let chunk_result = serde_json::from_slice::<OllamaStreamChunk>(trimmed)
                            .map(|c| StreamChunk {
                                content_delta: c.message.map(|m| m.content).unwrap_or_default(),
                                done: c.done,
                                prompt_tokens: c.prompt_eval_count.unwrap_or(0),
                                completion_tokens: c.eval_count.unwrap_or(0),
                            })
                            .map_err(|e| ProviderError::Parse(e.to_string()));
                        return Some((chunk_result, (byte_stream, buf)));
                    }

                    match byte_stream.next().await {
                        Some(Ok(bytes)) => buf.extend_from_slice(&bytes),
                        Some(Err(e)) => {
                            return Some((Err(ProviderError::Http(e)), (byte_stream, buf)))
                        }
                        None => {
                            if buf.is_empty() {
                                return None;
                            }
                            let trimmed = buf.trim_ascii().to_vec();
                            buf.clear();
                            if trimmed.is_empty() {
                                return None;
                            }
                            let chunk_result =
                                serde_json::from_slice::<OllamaStreamChunk>(&trimmed)
                                    .map(|c| StreamChunk {
                                        content_delta: c
                                            .message
                                            .map(|m| m.content)
                                            .unwrap_or_default(),
                                        done: c.done,
                                        prompt_tokens: c.prompt_eval_count.unwrap_or(0),
                                        completion_tokens: c.eval_count.unwrap_or(0),
                                    })
                                    .map_err(|e| ProviderError::Parse(e.to_string()));
                            return Some((chunk_result, (byte_stream, buf)));
                        }
                    }
                }
            },
        );

        Ok(Box::pin(stream))
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let resp = self
            .client
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Api(format!("{status}: {text}")));
        }

        let parsed: TagsResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;

        Ok(parsed
            .models
            .into_iter()
            .map(|m| ModelInfo {
                name: m.name,
                size_bytes: m.size,
            })
            .collect())
    }
}
