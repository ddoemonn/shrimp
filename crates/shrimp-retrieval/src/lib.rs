use std::path::PathBuf;

use shrimp_index::{IndexError, IndexStore, Symbol};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TraceEntry {
    pub name: String,
    pub file: String,
    pub line: u32,
    pub score: f32,
    pub kind: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RetrievalTrace {
    pub entries: Vec<TraceEntry>,
}

pub struct TextSearch {
    repo_root: PathBuf,
    max_matches: usize,
}

impl TextSearch {
    pub fn new(repo_root: PathBuf) -> Self {
        Self {
            repo_root,
            max_matches: 20,
        }
    }

    pub fn search(&self, query: &str) -> Vec<TraceEntry> {
        let re = regex::Regex::new(query).ok();
        let mut results: Vec<TraceEntry> = Vec::new();
        for entry in ignore::WalkBuilder::new(&self.repo_root).build().flatten() {
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }
            let path = entry.path().to_path_buf();
            let skip = path.components().any(|c| {
                let s = c.as_os_str().to_string_lossy();
                s == ".shrimp" || s == ".git" || s == "target"
            });
            if skip {
                continue;
            }
            let file_str = match path.to_str() {
                Some(s) => s.to_owned(),
                None => continue,
            };
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            for (i, line) in content.lines().enumerate() {
                let matched = re
                    .as_ref()
                    .map_or_else(|| line.contains(query), |r| r.is_match(line));
                if matched {
                    results.push(TraceEntry {
                        name: String::new(),
                        file: file_str.clone(),
                        line: (i + 1) as u32,
                        score: 0.5,
                        kind: "text".to_owned(),
                    });
                    if results.len() >= self.max_matches {
                        return results;
                    }
                }
            }
        }
        results
    }
}

pub struct RetrievalEngine {
    store: IndexStore,
    repo_root: PathBuf,
    searcher: TextSearch,
}

impl RetrievalEngine {
    pub fn new(store: IndexStore, repo_root: PathBuf) -> Self {
        let searcher = TextSearch::new(repo_root.clone());
        Self {
            store,
            repo_root,
            searcher,
        }
    }

    pub fn repo_root(&self) -> &std::path::Path {
        &self.repo_root
    }

    pub fn lookup_symbol(&self, query: &str) -> Result<Vec<Symbol>, IndexError> {
        self.store.lookup_symbol(query)
    }

    pub fn text_search(&self, query: &str) -> Vec<TraceEntry> {
        self.searcher.search(query)
    }

    pub fn build_trace(&self, query: &str) -> Result<RetrievalTrace, IndexError> {
        let sym_hits = self.store.lookup_symbol(query)?;
        let text_hits = self.searcher.search(query);

        let mut entries: Vec<TraceEntry> = sym_hits
            .iter()
            .map(|s| TraceEntry {
                name: s.name.clone(),
                file: s.file.clone(),
                line: s.line,
                score: 1.0,
                kind: "symbol".to_owned(),
            })
            .collect();

        entries.extend(text_hits.into_iter().take(3));

        Ok(RetrievalTrace { entries })
    }
}
