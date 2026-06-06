use serde::{Deserialize, Serialize};
use shrimp_index::IndexError;
use shrimp_retrieval::{RetrievalEngine, RetrievalTrace};

#[derive(Debug, Clone)]
pub struct ContextBudget {
    pub max_tokens: usize,
    pub used_tokens: usize,
}

impl ContextBudget {
    pub fn new(max_tokens: usize) -> Self {
        Self {
            max_tokens,
            used_tokens: 0,
        }
    }

    pub fn usage_pct(&self) -> f32 {
        self.used_tokens as f32 / self.max_tokens as f32 * 100.0
    }

    pub fn remaining(&self) -> usize {
        self.max_tokens.saturating_sub(self.used_tokens)
    }

    pub fn try_add(&mut self, tokens: usize) -> bool {
        if self.used_tokens + tokens <= self.max_tokens {
            self.used_tokens += tokens;
            true
        } else {
            false
        }
    }

    pub fn estimate_tokens(text: &str) -> usize {
        text.len() / 4 + 1
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextItem {
    pub label: String,
    pub content: String,
    pub tokens: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextBundle {
    pub items: Vec<ContextItem>,
    pub trace: RetrievalTrace,
}

impl ContextBundle {
    pub fn total_tokens(&self) -> usize {
        self.items.iter().map(|i| i.tokens).sum()
    }

    pub fn format_context(&self) -> String {
        self.items
            .iter()
            .map(|i| format!("[{}]\n{}", i.label, i.content))
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

pub struct ContextEngine {
    budget: ContextBudget,
    history: Vec<(String, String)>,
}

impl ContextEngine {
    pub fn new(max_tokens: usize) -> Self {
        Self {
            budget: ContextBudget::new(max_tokens),
            history: Vec::new(),
        }
    }

    pub fn add_history(&mut self, role: &str, content: &str) {
        self.history.push((role.to_string(), content.to_string()));
    }

    pub fn history(&self) -> &[(String, String)] {
        &self.history
    }

    pub fn reset_budget(&mut self) {
        self.budget.used_tokens = 0;
    }

    pub fn budget(&self) -> &ContextBudget {
        &self.budget
    }

    pub fn build_for_query(
        &mut self,
        query: &str,
        retrieval: &RetrievalEngine,
    ) -> Result<ContextBundle, IndexError> {
        self.budget.used_tokens = 0;
        let trace = retrieval.build_trace(query)?;
        let mut bundle = ContextBundle {
            items: Vec::new(),
            trace: trace.clone(),
        };

        for entry in &trace.entries {
            let label = format!("symbol: {} ({}:{})", entry.name, entry.file, entry.line);
            let content = format!("{} at {}:{}", entry.name, entry.file, entry.line);
            let tokens = ContextBudget::estimate_tokens(&content);
            if self.budget.try_add(tokens) {
                bundle.items.push(ContextItem {
                    label,
                    content,
                    tokens,
                });
            }
        }

        Ok(bundle)
    }
}
