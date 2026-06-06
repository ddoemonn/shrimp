use chrono::Utc;
use serde::{Deserialize, Serialize};
use shrimp_provider::ChatMessage;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub messages: Vec<ChatMessage>,
}

impl Session {
    pub fn new() -> Self {
        Self {
            id: Utc::now().format("%Y%m%d_%H%M%S").to_string(),
            messages: Vec::new(),
        }
    }

    pub fn add_message(&mut self, role: &str, content: &str) {
        self.messages.push(ChatMessage {
            role: role.to_string(),
            content: content.to_string(),
        });
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}.json", self.id));
        let json =
            serde_json::to_string_pretty(self).map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(path, json)
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct FileSnapshot {
    pub path: PathBuf,
    pub prior_content: Option<Vec<u8>>,
}

#[derive(Debug, Default)]
pub struct SnapshotManager {
    snapshots: Vec<FileSnapshot>,
}

impl SnapshotManager {
    pub fn begin_turn(&mut self) {
        self.snapshots.clear();
    }

    pub fn record_file(&mut self, path: &Path, repo_root: &Path) {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            repo_root.join(path)
        };
        let prior_content = std::fs::read(&abs).ok();
        self.snapshots.push(FileSnapshot {
            path: abs,
            prior_content,
        });
    }

    pub fn undo(&self, _repo_root: &Path) -> std::io::Result<()> {
        for snap in self.snapshots.iter().rev() {
            match &snap.prior_content {
                Some(content) => std::fs::write(&snap.path, content)?,
                None => {
                    let _ = std::fs::remove_file(&snap.path);
                }
            }
        }
        Ok(())
    }
}
