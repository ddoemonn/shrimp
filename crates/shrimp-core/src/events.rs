use serde::{Deserialize, Serialize};
use shrimp_retrieval::RetrievalTrace;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentEvent {
    Thinking,
    AssistantDelta {
        text: String,
    },
    AssistantClear,
    ToolStart {
        name: String,
        args: String,
    },
    ToolEnd {
        name: String,
        output: String,
        duration_ms: u64,
        success: bool,
    },
    RetrievalTrace {
        trace: RetrievalTrace,
    },
    TokenUpdate {
        prompt: u32,
        completion: u32,
        budget_pct: f32,
    },
    DiffPreview {
        path: String,
        old_content: String,
        new_content: String,
    },
    TurnComplete {
        response: String,
    },
    Error {
        message: String,
    },
    IndexReady {
        symbols: usize,
        files: usize,
        duration_ms: u64,
    },
}
