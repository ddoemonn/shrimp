use serde::{Deserialize, Serialize};
use shrimp_provider::{ProviderConfig, ProviderKind};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShrimpConfig {
    pub repo_root: PathBuf,
    pub provider: ProviderKind,
    pub base_url: String,
    pub model: String,
    pub auto_approve: bool,
    pub max_context_tokens: usize,
}

impl Default for ShrimpConfig {
    fn default() -> Self {
        Self {
            repo_root: PathBuf::from("."),
            provider: ProviderKind::Ollama,
            base_url: "http://localhost:11434".to_string(),
            model: "qwen2.5-coder:7b".to_string(),
            auto_approve: true,
            max_context_tokens: 8192,
        }
    }
}

impl ShrimpConfig {
    pub fn load(repo_root: &Path) -> Self {
        let config_path = repo_root.join(".shrimp").join("config.toml");
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            if let Ok(cfg) = toml::from_str::<ShrimpConfig>(&content) {
                return cfg;
            }
        }
        ShrimpConfig {
            repo_root: repo_root.to_path_buf(),
            ..ShrimpConfig::default()
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let dir = self.repo_root.join(".shrimp");
        std::fs::create_dir_all(&dir)?;
        let content = toml::to_string_pretty(self).unwrap_or_default();
        std::fs::write(dir.join("config.toml"), content)
    }

    pub fn provider_config(&self) -> ProviderConfig {
        ProviderConfig::new(
            self.provider.clone(),
            self.base_url.clone(),
            self.model.clone(),
        )
    }

    pub fn index_dir(&self) -> PathBuf {
        self.repo_root.join(".shrimp").join("index")
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.repo_root.join(".shrimp").join("sessions")
    }

    pub fn snapshots_dir(&self) -> PathBuf {
        self.repo_root.join(".shrimp").join("snapshots")
    }
}
