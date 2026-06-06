use anyhow::Result;
use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde_json::Value;
use shrimp_retrieval::RetrievalEngine;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

pub struct ToolContext {
    pub repo_root: PathBuf,
}

impl ToolContext {
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root }
    }

    pub fn resolve(&self, path: &str) -> PathBuf {
        let p = Path::new(path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.repo_root.join(p)
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub duration_ms: u64,
}

impl ToolResult {
    pub fn ok(output: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            success: true,
            output: output.into(),
            duration_ms,
        }
    }

    pub fn err(output: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            success: false,
            output: output.into(),
            duration_ms,
        }
    }
}

struct GuardState {
    read_files: HashSet<String>,
    write_attempts: HashMap<String, u32>,
    recent_calls: Vec<(String, String)>,
}

static GUARD: Lazy<Mutex<GuardState>> = Lazy::new(|| {
    Mutex::new(GuardState {
        read_files: HashSet::new(),
        write_attempts: HashMap::new(),
        recent_calls: Vec::new(),
    })
});

pub fn mark_read(path: &str) {
    GUARD.lock().unwrap().read_files.insert(path.to_string());
}

pub fn has_read(path: &str) -> bool {
    GUARD.lock().unwrap().read_files.contains(path)
}

pub fn reset_session_guards() {
    let mut g = GUARD.lock().unwrap();
    g.read_files.clear();
    g.write_attempts.clear();
    g.recent_calls.clear();
}

pub fn reset_round_dedup() {
    GUARD.lock().unwrap().recent_calls.clear();
}

pub fn guard_write(path: &str, ctx: &ToolContext) -> Option<String> {
    let resolved = ctx.resolve(path);
    if !resolved.exists() {
        return None;
    }
    if has_read(path) {
        return None;
    }
    let mut g = GUARD.lock().unwrap();
    let attempts = g.write_attempts.entry(path.to_string()).or_insert(0);
    *attempts += 1;
    if *attempts <= 1 {
        Some(format!(
            "read-before-write guard: read '{}' first, then retry",
            path
        ))
    } else {
        None
    }
}

pub fn is_duplicate(name: &str, args: &str) -> bool {
    let g = GUARD.lock().unwrap();
    g.recent_calls.iter().any(|(n, a)| n == name && a == args)
}

pub fn mark_call(name: &str, args: &str) {
    GUARD
        .lock()
        .unwrap()
        .recent_calls
        .push((name.to_string(), args.to_string()));
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    async fn execute(
        &self,
        args: Value,
        ctx: &ToolContext,
        retrieval: &RetrievalEngine,
    ) -> Result<ToolResult>;
}

fn get_str<'a>(args: &'a Value, keys: &[&str]) -> Option<&'a str> {
    for k in keys {
        if let Some(v) = args.get(k).and_then(|v| v.as_str()) {
            return Some(v);
        }
    }
    None
}

pub struct ReadFileTool;
pub struct WriteFileTool;
pub struct PatchTool;
pub struct BashTool;
pub struct SearchTool;
pub struct SymbolLookupTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }
    fn description(&self) -> &str {
        "read_file(path) — read a file. Example: {\"name\":\"read_file\",\"arguments\":{\"path\":\"src/main.rs\"}}"
    }

    async fn execute(
        &self,
        args: Value,
        ctx: &ToolContext,
        _retrieval: &RetrievalEngine,
    ) -> Result<ToolResult> {
        let t = Instant::now();
        let path = get_str(&args, &["path", "file_path", "filename", "file"])
            .ok_or_else(|| anyhow::anyhow!("missing path"))?
            .to_string();
        let resolved = ctx.resolve(&path);
        match tokio::fs::read_to_string(&resolved).await {
            Ok(content) => {
                mark_read(&path);
                Ok(ToolResult::ok(content, t.elapsed().as_millis() as u64))
            }
            Err(e) => Ok(ToolResult::err(
                e.to_string(),
                t.elapsed().as_millis() as u64,
            )),
        }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }
    fn description(&self) -> &str {
        "write_file(path, content) — create or overwrite a file. Example: {\"name\":\"write_file\",\"arguments\":{\"path\":\"hello.py\",\"content\":\"print(1)\"}}"
    }

    async fn execute(
        &self,
        args: Value,
        ctx: &ToolContext,
        _retrieval: &RetrievalEngine,
    ) -> Result<ToolResult> {
        let t = Instant::now();
        let path = get_str(&args, &["path", "file_path", "filename", "file"])
            .ok_or_else(|| anyhow::anyhow!("missing path"))?
            .to_string();
        let content = get_str(&args, &["content", "contents", "text", "body"])
            .ok_or_else(|| anyhow::anyhow!("missing content"))?
            .to_string();
        if let Some(msg) = guard_write(&path, ctx) {
            return Ok(ToolResult::err(msg, t.elapsed().as_millis() as u64));
        }
        let resolved = ctx.resolve(&path);
        if let Some(parent) = resolved.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&resolved, &content).await?;
        mark_read(&path);
        Ok(ToolResult::ok(
            format!("wrote {}", path),
            t.elapsed().as_millis() as u64,
        ))
    }
}

#[async_trait]
impl Tool for PatchTool {
    fn name(&self) -> &str {
        "patch"
    }
    fn description(&self) -> &str {
        "patch(path, old_str, new_str) — replace exact text in a file. Read the file first."
    }

    async fn execute(
        &self,
        args: Value,
        ctx: &ToolContext,
        _retrieval: &RetrievalEngine,
    ) -> Result<ToolResult> {
        let t = Instant::now();
        let path = get_str(&args, &["path", "file_path", "filename", "file"])
            .ok_or_else(|| anyhow::anyhow!("missing path"))?
            .to_string();
        let old_str = get_str(&args, &["old_str", "old", "search", "original"])
            .ok_or_else(|| anyhow::anyhow!("missing old_str"))?
            .to_string();
        let new_str = get_str(&args, &["new_str", "new", "replace", "replacement"])
            .ok_or_else(|| anyhow::anyhow!("missing new_str"))?
            .to_string();
        if let Some(msg) = guard_write(&path, ctx) {
            return Ok(ToolResult::err(msg, t.elapsed().as_millis() as u64));
        }
        let resolved = ctx.resolve(&path);
        let content = match tokio::fs::read_to_string(&resolved).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult::err(
                    e.to_string(),
                    t.elapsed().as_millis() as u64,
                ))
            }
        };
        let count = content.matches(old_str.as_str()).count();
        if count != 1 {
            return Ok(ToolResult::err(
                format!("expected 1 occurrence of old_str, found {}", count),
                t.elapsed().as_millis() as u64,
            ));
        }
        let new_content = content.replacen(old_str.as_str(), new_str.as_str(), 1);
        tokio::fs::write(&resolved, &new_content).await?;
        mark_read(&path);
        Ok(ToolResult::ok(
            format!("patched {}", path),
            t.elapsed().as_millis() as u64,
        ))
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }
    fn description(&self) -> &str {
        "bash(command) — run a shell command. Example: {\"name\":\"bash\",\"arguments\":{\"command\":\"echo hello > file.txt\"}}"
    }

    async fn execute(
        &self,
        args: Value,
        ctx: &ToolContext,
        _retrieval: &RetrievalEngine,
    ) -> Result<ToolResult> {
        let t = Instant::now();
        let command = get_str(&args, &["command", "cmd", "script", "shell"])
            .ok_or_else(|| anyhow::anyhow!("missing command"))?
            .to_string();
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
            .current_dir(&ctx.repo_root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let combined = match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
            (false, false) => format!("{}\n{}", stdout.trim_end(), stderr.trim_end()),
            (false, true) => stdout,
            (true, false) => stderr,
            (true, true) => String::new(),
        };
        let truncated = if combined.len() > 2000 {
            combined[..2000].to_string()
        } else {
            combined
        };
        let duration_ms = t.elapsed().as_millis() as u64;
        if output.status.success() {
            Ok(ToolResult::ok(truncated, duration_ms))
        } else {
            Ok(ToolResult::err(truncated, duration_ms))
        }
    }
}

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }
    fn description(&self) -> &str {
        "search(query) — find text in project files"
    }

    async fn execute(
        &self,
        args: Value,
        _ctx: &ToolContext,
        retrieval: &RetrievalEngine,
    ) -> Result<ToolResult> {
        let t = Instant::now();
        let query = get_str(&args, &["query", "pattern", "text", "search"])
            .ok_or_else(|| anyhow::anyhow!("missing query"))?
            .to_string();
        let entries = retrieval.text_search(&query);
        if entries.is_empty() {
            return Ok(ToolResult::ok("no results", t.elapsed().as_millis() as u64));
        }
        let lines: Vec<String> = entries
            .iter()
            .map(|e| format!("{}:{}: {}", e.file, e.line, e.name))
            .collect();
        Ok(ToolResult::ok(
            lines.join("\n"),
            t.elapsed().as_millis() as u64,
        ))
    }
}

#[async_trait]
impl Tool for SymbolLookupTool {
    fn name(&self) -> &str {
        "symbol_lookup"
    }
    fn description(&self) -> &str {
        "symbol_lookup(query) — find functions/classes by name in the index"
    }

    async fn execute(
        &self,
        args: Value,
        _ctx: &ToolContext,
        retrieval: &RetrievalEngine,
    ) -> Result<ToolResult> {
        let t = Instant::now();
        let query = get_str(&args, &["query", "name", "symbol", "search"])
            .ok_or_else(|| anyhow::anyhow!("missing query"))?
            .to_string();
        let symbols = retrieval.lookup_symbol(&query)?;
        if symbols.is_empty() {
            return Ok(ToolResult::ok(
                "no symbols found",
                t.elapsed().as_millis() as u64,
            ));
        }
        let lines: Vec<String> = symbols
            .iter()
            .map(|s| format!("{} ({}) at {}:{}", s.name, s.kind.as_str(), s.file, s.line))
            .collect();
        Ok(ToolResult::ok(
            lines.join("\n"),
            t.elapsed().as_millis() as u64,
        ))
    }
}

pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        let tools: Vec<Box<dyn Tool>> = vec![
            Box::new(ReadFileTool),
            Box::new(WriteFileTool),
            Box::new(PatchTool),
            Box::new(BashTool),
            Box::new(SearchTool),
            Box::new(SymbolLookupTool),
        ];
        Self { tools }
    }
}

impl ToolRegistry {
    pub fn tool_descriptions(&self) -> Vec<String> {
        self.tools
            .iter()
            .map(|t| format!("- {}: {}", t.name(), t.description()))
            .collect()
    }

    pub async fn execute(
        &self,
        name: &str,
        args: Value,
        ctx: &ToolContext,
        retrieval: &RetrievalEngine,
    ) -> Result<ToolResult> {
        let args_str = args.to_string();
        if is_duplicate(name, &args_str) {
            return Ok(ToolResult::err("duplicate tool call skipped", 0));
        }
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {}", name))?;
        let result = tool.execute(args, ctx, retrieval).await?;
        if result.success {
            mark_call(name, &args_str);
        }
        Ok(result)
    }

    pub fn tools(&self) -> &[Box<dyn Tool>] {
        &self.tools
    }
}
