use std::path::Path;
use std::time::Instant;

use crossbeam_channel::Sender;
use futures_util::StreamExt;
use shrimp_context::ContextEngine;
use shrimp_index::{build_index, IndexError, IndexStore};
use shrimp_provider::{create_provider, AnyProvider, ChatMessage, ChatRequest, ProviderError};
use shrimp_retrieval::RetrievalEngine;
use shrimp_tools::{reset_session_guards, ToolContext, ToolRegistry};

use crate::config::ShrimpConfig;
use crate::events::AgentEvent;
use crate::parser::ToolCallParser;
use crate::session::{Session, SnapshotManager};

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub struct Agent {
    config: ShrimpConfig,
    provider: AnyProvider,
    registry: ToolRegistry,
    retrieval: RetrievalEngine,
    context_engine: ContextEngine,
    session: Session,
    snapshot: SnapshotManager,
    event_tx: Sender<AgentEvent>,
}

struct StreamedResponse {
    content: String,
    prompt_tokens: u32,
    completion_tokens: u32,
    finish_reason: Option<String>,
}

fn emit(tx: &Sender<AgentEvent>, ev: AgentEvent) {
    let _ = tx.try_send(ev);
}

fn user_wants_action(msg: &str) -> bool {
    let m = msg.to_lowercase();
    [
        "implement", "write", "make", "build", "create", "add", "fix", "refactor",
        "do it", "go ahead", "proceed", "apply", "change", "update", "convert",
        "rewrite", "develop", "happening", "some code", "write code", "write the",
    ]
    .iter()
    .any(|w| m.contains(w))
}

fn action_requested(user_message: &str, history: &[(String, String)]) -> bool {
    if user_wants_action(user_message) {
        return true;
    }
    history
        .iter()
        .rev()
        .take(6)
        .filter(|(role, _)| role == "user")
        .any(|(_, content)| user_wants_action(content))
}

fn looks_like_plan_only(text: &str) -> bool {
    let t = text.to_lowercase();
    [
        "next step",
        "will involve",
        "to achieve this",
        "to fulfill",
        "will require",
        "this will involve",
        "involves refactoring",
        "the existing code",
        "located in",
    ]
    .iter()
    .any(|p| t.contains(p))
        && !text.contains("```json")
}

async fn collect_stream(
    provider: &AnyProvider,
    req: ChatRequest,
    tx: &Sender<AgentEvent>,
) -> Result<StreamedResponse, ProviderError> {
    let mut stream = provider.chat_stream(req).await?;
    let mut content = String::new();
    let mut prompt_tokens = 0u32;
    let mut completion_tokens = 0u32;
    let mut finish_reason = None;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if !chunk.content_delta.is_empty() {
            content.push_str(&chunk.content_delta);
            emit(
                tx,
                AgentEvent::AssistantDelta {
                    text: chunk.content_delta,
                },
            );
        }
        if chunk.prompt_tokens > 0 {
            prompt_tokens = chunk.prompt_tokens;
        }
        if chunk.completion_tokens > 0 {
            completion_tokens = chunk.completion_tokens;
        }
        if chunk.done {
            finish_reason = Some("stop".to_string());
        }
    }
    Ok(StreamedResponse {
        content,
        prompt_tokens,
        completion_tokens,
        finish_reason,
    })
}

impl Agent {
    pub fn new(config: ShrimpConfig, event_tx: Sender<AgentEvent>) -> Result<Self, AgentError> {
        reset_session_guards();
        std::fs::create_dir_all(config.index_dir())?;
        let stats = build_index(&config.repo_root, &config.index_dir())?;
        let store = IndexStore::open(&config.index_dir().join("index.redb"))?;
        let retrieval = RetrievalEngine::new(store, config.repo_root.clone());
        emit(
            &event_tx,
            AgentEvent::IndexReady {
                symbols: stats.symbols_total,
                files: stats.files_total,
                duration_ms: stats.duration_ms,
            },
        );
        let provider = create_provider(&config.provider_config())?;
        let context_engine = ContextEngine::new(config.max_context_tokens);
        Ok(Self {
            config,
            provider,
            registry: ToolRegistry::default(),
            retrieval,
            context_engine,
            session: Session::new(),
            snapshot: SnapshotManager::default(),
            event_tx,
        })
    }

    pub fn reindex(&mut self) -> Result<(), AgentError> {
        let stats = build_index(&self.config.repo_root, &self.config.index_dir())?;
        let store = IndexStore::open(&self.config.index_dir().join("index.redb"))?;
        self.retrieval = RetrievalEngine::new(store, self.config.repo_root.clone());
        emit(
            &self.event_tx,
            AgentEvent::IndexReady {
                symbols: stats.symbols_total,
                files: stats.files_total,
                duration_ms: stats.duration_ms,
            },
        );
        Ok(())
    }

    pub fn undo_last_edit(&mut self) -> Result<(), AgentError> {
        self.snapshot.undo(&self.config.repo_root)?;
        Ok(())
    }

    pub fn config(&self) -> &ShrimpConfig {
        &self.config
    }

    pub fn run_turn(&mut self, user_message: &str) -> Result<String, AgentError> {
        self.snapshot.begin_turn();
        self.session.add_message("user", user_message);
        self.context_engine.add_history("user", user_message);

        let bundle = self
            .context_engine
            .build_for_query(user_message, &self.retrieval)?;
        emit(
            &self.event_tx,
            AgentEvent::RetrievalTrace {
                trace: bundle.trace.clone(),
            },
        );

        let tool_descriptions = self.registry.tool_descriptions().join("\n");
        let repo = self.config.repo_root.display();

        let system_prompt = format!(
            "You are shrimp, a coding agent working in the repository at: {}\n\
             \n\
             TOOL USAGE: emit exactly one JSON object in a fenced ```json block per tool call:\n\
             ```json\n{{\"name\":\"tool_name\",\"arguments\":{{\"key\":\"value\"}}}}\n```\n\
             \n\
             Tools:\n{}\n\
             \n\
             RULES:\n\
             - When asked what a project is or what it does, use read_file or search first, then answer.\n\
             - When asked to implement, build, create, refactor, or write code: DO IT with write_file or patch. Never reply with only a plan.\n\
             - Forbidden without a tool call: \"next step\", \"will involve\", \"to achieve this\", \"will require refactoring\".\n\
             - After reading files for an implementation request, your very next message MUST be a write_file or patch tool call.\n\
             - To create a new file use write_file. To change an existing file, read_file it first, then patch or write_file.\n\
             - patch old_str must match the file exactly once. If it fails, read_file again and retry with a unique snippet.\n\
             - Emit one tool call at a time and wait for its result before the next.\n\
             - When finished editing, briefly show what changed (paths + key diffs). Never say only \"task complete\" or \"done\".\n\
             - Be concise. Answer based on what you actually find in the files.\n\
             \n\
             Relevant symbols found:\n{}",
            repo,
            tool_descriptions,
            bundle.format_context()
        );

        let mut messages: Vec<ChatMessage> = vec![ChatMessage::system(&system_prompt)];
        for (role, content) in self.context_engine.history() {
            messages.push(ChatMessage {
                role: role.clone(),
                content: content.clone(),
            });
        }

        let ctx = ToolContext::new(self.config.repo_root.clone());
        let rt = tokio::runtime::Handle::try_current();
        let mut final_response = String::new();
        let wants_action = action_requested(user_message, self.context_engine.history());
        let mut did_write = false;
        let mut plan_nudges = 0u8;

        for _ in 0..12usize {
            shrimp_tools::reset_round_dedup();
            emit(&self.event_tx, AgentEvent::Thinking);

            let req = ChatRequest {
                messages: messages.clone(),
                temperature: 0.1,
                max_tokens: Some(2048),
            };

            let response = match &rt {
                Ok(handle) => tokio::task::block_in_place(|| {
                    handle.block_on(collect_stream(&self.provider, req, &self.event_tx))
                })
                .map_err(|e| AgentError::Message(e.to_string()))?,
                Err(_) => {
                    let rt2 = tokio::runtime::Runtime::new()?;
                    rt2.block_on(collect_stream(&self.provider, req, &self.event_tx))
                        .map_err(|e| AgentError::Message(e.to_string()))?
                }
            };

            emit(
                &self.event_tx,
                AgentEvent::TokenUpdate {
                    prompt: response.prompt_tokens,
                    completion: response.completion_tokens,
                    budget_pct: self.context_engine.budget().usage_pct(),
                },
            );

            if response.content.is_empty() && response.finish_reason.as_deref() == Some("length") {
                emit(&self.event_tx, AgentEvent::AssistantClear);
                messages.push(ChatMessage::user(
                    "Your response was truncated. Please continue from where you left off.",
                ));
                continue;
            }

            let tool_calls = ToolCallParser::parse(&response.content);

            if tool_calls.is_empty() {
                if wants_action && !did_write && plan_nudges < 3 {
                    plan_nudges += 1;
                    emit(&self.event_tx, AgentEvent::AssistantClear);
                    messages.push(ChatMessage::assistant(&response.content));
                    let nudge = if looks_like_plan_only(&response.content) {
                        "Stop planning. Emit a write_file or patch tool call NOW in a ```json block with the full code. Do not describe what you will do — write the files."
                    } else {
                        "The user asked for code changes. Emit a write_file or patch tool call in a ```json block. No text-only replies until files are written."
                    };
                    messages.push(ChatMessage::user(nudge));
                    continue;
                }
                final_response = response.content.clone();
                self.session.add_message("assistant", &response.content);
                self.context_engine
                    .add_history("assistant", &response.content);
                break;
            }

            emit(&self.event_tx, AgentEvent::AssistantClear);
            messages.push(ChatMessage::assistant(&response.content));

            let mut tool_results_text = String::new();

            for tc in &tool_calls {
                emit(
                    &self.event_tx,
                    AgentEvent::ToolStart {
                        name: tc.name.clone(),
                        args: tc.arguments.to_string(),
                    },
                );

                let t0 = Instant::now();

                if tc.name == "write_file" || tc.name == "patch" {
                    let path_keys = ["path", "file_path", "filename", "file"];
                    let path_val = path_keys
                        .iter()
                        .find_map(|k| tc.arguments.get(*k).and_then(|v| v.as_str()));
                    if let Some(path_str) = path_val {
                        self.snapshot
                            .record_file(Path::new(path_str), &self.config.repo_root.clone());
                        if tc.name == "patch" {
                            let old = tc
                                .arguments
                                .get("old_str")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let new = tc
                                .arguments
                                .get("new_str")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            emit(
                                &self.event_tx,
                                AgentEvent::DiffPreview {
                                    path: path_str.to_string(),
                                    old_content: old,
                                    new_content: new,
                                },
                            );
                        }
                    }
                }

                let retrieval = &self.retrieval;
                let result = match &rt {
                    Ok(handle) => tokio::task::block_in_place(|| {
                        handle.block_on(self.registry.execute(
                            &tc.name,
                            tc.arguments.clone(),
                            &ctx,
                            retrieval,
                        ))
                    })
                    .map_err(|e| AgentError::Message(e.to_string()))?,
                    Err(_) => {
                        let rt2 = tokio::runtime::Runtime::new()?;
                        rt2.block_on(self.registry.execute(
                            &tc.name,
                            tc.arguments.clone(),
                            &ctx,
                            retrieval,
                        ))
                        .map_err(|e| AgentError::Message(e.to_string()))?
                    }
                };

                let elapsed = t0.elapsed().as_millis() as u64;

                emit(
                    &self.event_tx,
                    AgentEvent::ToolEnd {
                        name: tc.name.clone(),
                        output: result.output.clone(),
                        duration_ms: elapsed,
                        success: result.success,
                    },
                );

                if result.success && (tc.name == "write_file" || tc.name == "patch") {
                    did_write = true;
                }

                let max_len = if tc.name == "read_file" { 4000 } else { 1500 };
                let snippet = if result.output.len() > max_len {
                    &result.output[..max_len]
                } else {
                    &result.output
                };
                tool_results_text
                    .push_str(&format!("Tool '{}' result:\n{}\n---\n", tc.name, snippet));
            }

            messages.push(ChatMessage::user(&tool_results_text));
        }

        if final_response.is_empty() {
            emit(&self.event_tx, AgentEvent::Thinking);
            messages.push(ChatMessage::user(
                "Summarize for the user what you found or changed. Use concrete details from tool results. No tool calls.",
            ));
            let req = ChatRequest {
                messages: messages.clone(),
                temperature: 0.1,
                max_tokens: Some(2048),
            };
            let summary = match &rt {
                Ok(handle) => tokio::task::block_in_place(|| {
                    handle.block_on(collect_stream(&self.provider, req, &self.event_tx))
                }),
                Err(_) => {
                    let rt2 = tokio::runtime::Runtime::new()?;
                    rt2.block_on(collect_stream(&self.provider, req, &self.event_tx))
                }
            };
            if let Ok(response) = summary {
                if ToolCallParser::parse(&response.content).is_empty() {
                    let text = response.content.trim();
                    if !text.is_empty() {
                        final_response = response.content.clone();
                        self.session.add_message("assistant", &response.content);
                        self.context_engine
                            .add_history("assistant", &response.content);
                    }
                }
            }
        }

        emit(
            &self.event_tx,
            AgentEvent::TurnComplete {
                response: final_response.clone(),
            },
        );

        let sessions_dir = self.config.sessions_dir();
        let _ = self.session.save(&sessions_dir);

        Ok(final_response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_action_request() {
        assert!(user_wants_action("okay make this happen write some code"));
        assert!(user_wants_action("implement a ratatui tui"));
        assert!(!user_wants_action("what is this project?"));
    }

    #[test]
    fn detects_plan_only_reply() {
        let plan = "The next step will involve refactoring src/main.rs to use ratatui.";
        assert!(looks_like_plan_only(plan));
        assert!(!looks_like_plan_only("Updated src/main.rs with ratatui UI."));
    }
}
