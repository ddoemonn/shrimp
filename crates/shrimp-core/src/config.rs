use serde::{Deserialize, Serialize};
use shrimp_provider::{ProviderConfig, ProviderKind};
use std::path::{Path, PathBuf};

fn default_repo_root() -> PathBuf {
    PathBuf::from(".")
}

fn default_provider() -> ProviderKind {
    ProviderKind::Ollama
}

fn default_base_url() -> String {
    String::new()
}

fn default_model() -> String {
    "qwen2.5-coder:7b".to_string()
}

fn default_auto_approve() -> bool {
    true
}

fn default_max_context_tokens() -> usize {
    8192
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShrimpConfig {
    #[serde(skip, default = "default_repo_root")]
    pub repo_root: PathBuf,
    #[serde(default = "default_provider")]
    pub provider: ProviderKind,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_auto_approve")]
    pub auto_approve: bool,
    #[serde(default = "default_max_context_tokens")]
    pub max_context_tokens: usize,
}

impl Default for ShrimpConfig {
    fn default() -> Self {
        let provider = ProviderKind::Ollama;
        let base_url = provider.resolve_base_url(None);
        Self {
            repo_root: default_repo_root(),
            provider,
            base_url,
            model: default_model(),
            auto_approve: default_auto_approve(),
            max_context_tokens: default_max_context_tokens(),
        }
    }
}

impl ShrimpConfig {
    pub fn load(repo_root: &Path) -> Self {
        let config_path = repo_root.join(".shrimp").join("config.toml");
        let mut cfg = if let Ok(content) = std::fs::read_to_string(&config_path) {
            if let Ok(mut c) = toml::from_str::<ShrimpConfig>(&content) {
                c.repo_root = repo_root.to_path_buf();
                c
            } else {
                ShrimpConfig {
                    repo_root: repo_root.to_path_buf(),
                    ..ShrimpConfig::default()
                }
            }
        } else {
            ShrimpConfig {
                repo_root: repo_root.to_path_buf(),
                ..ShrimpConfig::default()
            }
        };

        if cfg.base_url.is_empty() {
            cfg.base_url = cfg.provider.resolve_base_url(None);
        } else {
            cfg.base_url = cfg.provider.resolve_base_url(Some(&cfg.base_url));
        }

        cfg
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
