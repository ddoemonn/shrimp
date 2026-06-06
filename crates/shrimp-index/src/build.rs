use std::path::Path;
use std::time::Instant;

use ignore::WalkBuilder;
use sha2::{Digest, Sha256};

use crate::store::IndexStore;
use crate::{parser, FileMeta, IndexError, IndexStats};

pub fn build_index(repo_root: &Path, index_dir: &Path) -> Result<IndexStats, IndexError> {
    let db_path = index_dir.join("index.redb");
    let store = IndexStore::open(&db_path)?;
    let start = Instant::now();
    let mut stats = IndexStats::default();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for result in WalkBuilder::new(repo_root).build() {
        let entry = result?;
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path().to_path_buf();
        let skip = path.components().any(|c| {
            let s = c.as_os_str().to_string_lossy();
            s == ".marrow" || s == ".shrimp"
        });
        if skip {
            continue;
        }
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_owned(),
            None => continue,
        };
        if !parser::supported_extension(&ext) {
            continue;
        }
        let file_str = match path.to_str() {
            Some(s) => s.to_owned(),
            None => continue,
        };
        let content = match std::fs::read(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let hash: String = {
            let mut hasher = Sha256::new();
            hasher.update(&content);
            hasher
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        seen.insert(file_str.clone());
        stats.files_total += 1;
        if let Some(meta) = store.get_file_meta(&file_str)? {
            if meta.hash == hash {
                continue;
            }
        }
        stats.files_changed += 1;
        let source = match std::str::from_utf8(&content) {
            Ok(s) => s.to_owned(),
            Err(_) => continue,
        };
        let symbols = parser::parse_file(&file_str, &source, &ext);
        let mtime = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let sym_count = symbols.len();
        let meta = FileMeta {
            file: file_str.clone(),
            hash,
            mtime,
            symbol_count: sym_count,
        };
        store.upsert_file(&meta, &symbols)?;
        stats.symbols_total += sym_count;
    }

    let stored_files = store.all_files()?;
    for fm in stored_files {
        if !seen.contains(&fm.file) {
            store.remove_file(&fm.file)?;
            stats.files_removed += 1;
        }
    }

    stats.duration_ms = start.elapsed().as_millis() as u64;
    Ok(stats)
}
