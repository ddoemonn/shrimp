use anyhow::Result;
use crossbeam_channel::{unbounded, Receiver, Sender};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
    },
    Frame, Terminal,
};
use shrimp_core::{Agent, AgentEvent, ShrimpConfig, ToolCallParser};
use shrimp_provider::{list_models, ModelInfo, ProviderKind};
use serde_json::Value;
use std::io;
use std::sync::mpsc;

mod highlight;
mod markdown;

#[derive(Clone, PartialEq)]
enum ChatLineKind {
    User,
    Agent,
    Tool,
    ToolOk,
    ToolErr,
    System,
    Error,
}

#[derive(Clone)]
enum ChatContent {
    Text(String),
    Rich(Vec<Line<'static>>),
}

#[derive(Clone)]
struct ChatLine {
    kind: ChatLineKind,
    content: ChatContent,
}

impl ChatLine {
    fn text(s: impl Into<String>, kind: ChatLineKind) -> Self {
        Self {
            kind,
            content: ChatContent::Text(s.into()),
        }
    }

    fn rich(lines: Vec<Line<'static>>, kind: ChatLineKind) -> Self {
        Self {
            kind,
            content: ChatContent::Rich(lines),
        }
    }
}

struct DiffOverlay {
    path: String,
    old_content: String,
    new_content: String,
}

enum Screen {
    ProviderSelect,
    ModelSelect,
    Chat,
}

struct App {
    screen: Screen,
    should_quit: bool,

    provider_cursor: usize,

    models: Vec<ModelInfo>,
    model_cursor: usize,
    model_list_state: ListState,
    model_error: Option<String>,
    model_loading: bool,
    model_thread_rx: Option<mpsc::Receiver<Result<Vec<ModelInfo>, String>>>,

    messages: Vec<ChatLine>,
    live_assistant: String,
    input: String,
    scroll_offset: usize,
    view_rows: usize,
    view_height: usize,
    agent_running: bool,
    diff_overlay: Option<DiffOverlay>,

    config: ShrimpConfig,
    agent: Option<Agent>,
    agent_build_rx: Option<mpsc::Receiver<Result<Agent, String>>>,
    agent_turn_rx: Option<mpsc::Receiver<(Agent, Result<String, String>)>>,

    event_rx: Option<Receiver<AgentEvent>>,
    event_tx_agent: Option<Sender<AgentEvent>>,

    status_tokens: String,
    status_ctx_pct: f32,
    status_index: String,

    spinner_tick: usize,
    last_file_path: Option<String>,
    stream_tool_persisted: bool,
}

impl App {
    fn new(config: ShrimpConfig) -> Self {
        Self {
            screen: Screen::ProviderSelect,
            should_quit: false,
            provider_cursor: 0,
            models: Vec::new(),
            model_cursor: 0,
            model_list_state: ListState::default(),
            model_error: None,
            model_loading: false,
            model_thread_rx: None,
            messages: Vec::new(),
            live_assistant: String::new(),
            input: String::new(),
            scroll_offset: usize::MAX,
            view_rows: 0,
            view_height: 1,
            agent_running: false,
            diff_overlay: None,
            config,
            agent: None,
            agent_build_rx: None,
            agent_turn_rx: None,
            event_rx: None,
            event_tx_agent: None,
            status_tokens: String::new(),
            status_ctx_pct: 0.0,
            status_index: String::from("not indexed"),
            spinner_tick: 0,
            last_file_path: None,
            stream_tool_persisted: false,
        }
    }
}

impl App {
    fn max_offset(&self) -> usize {
        self.view_rows.saturating_sub(self.view_height)
    }

    fn effective_offset(&self) -> usize {
        if self.scroll_offset == usize::MAX {
            self.max_offset()
        } else {
            self.scroll_offset.min(self.max_offset())
        }
    }

    fn scroll_up(&mut self, n: usize) {
        let cur = self.effective_offset();
        self.scroll_offset = cur.saturating_sub(n);
    }

    fn scroll_down(&mut self, n: usize) {
        let cur = self.effective_offset();
        let max = self.max_offset();
        let next = (cur + n).min(max);
        self.scroll_offset = if next >= max { usize::MAX } else { next };
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll_offset = usize::MAX;
    }
}

fn provider_entries() -> Vec<(ProviderKind, &'static str, &'static str)> {
    vec![
        (ProviderKind::Ollama, "Ollama", "http://localhost:11434"),
        (ProviderKind::LmStudio, "LM Studio", "http://localhost:1234"),
    ]
}

fn spawn_model_fetch(
    pc: shrimp_provider::ProviderConfig,
) -> mpsc::Receiver<Result<Vec<ModelInfo>, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("tokio rt");
        let result = rt.block_on(list_models(&pc)).map_err(|e| e.to_string());
        let _ = tx.send(result);
    });
    rx
}

fn spawn_agent_build(
    config: ShrimpConfig,
    event_tx: Sender<AgentEvent>,
) -> mpsc::Receiver<Result<Agent, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = Agent::new(config, event_tx).map_err(|e| e.to_string());
        let _ = tx.send(result);
    });
    rx
}

fn spawn_agent_turn(
    mut agent: Agent,
    prompt: String,
) -> mpsc::Receiver<(Agent, Result<String, String>)> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = agent.run_turn(&prompt).map_err(|e| e.to_string());
        let _ = tx.send((agent, result));
    });
    rx
}

pub async fn run_app(config: ShrimpConfig) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(config);

    let result = event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        app.spinner_tick = app.spinner_tick.wrapping_add(1);
        terminal.draw(|f| render(f, app))?;

        if event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                handle_key(app, key.code, key.modifiers);
            }
        }

        drain_events(app);
        check_background_tasks(app);

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

fn check_background_tasks(app: &mut App) {
    if let Some(rx) = &app.model_thread_rx {
        if let Ok(result) = rx.try_recv() {
            app.model_thread_rx = None;
            app.model_loading = false;
            match result {
                Ok(models) if models.is_empty() => {
                    app.model_error = Some(no_models_hint(&app.config.provider));
                }
                Ok(models) => {
                    app.models = models;
                    app.model_error = None;
                    app.model_list_state.select(Some(0));
                }
                Err(e) => {
                    app.model_error = Some(provider_error_hint(
                        &app.config.provider,
                        &app.config.base_url,
                        &e,
                    ));
                }
            }
        }
    }

    if let Some(rx) = &app.agent_build_rx {
        if let Ok(result) = rx.try_recv() {
            app.agent_build_rx = None;
            match result {
                Ok(agent) => {
                    app.agent = Some(agent);
                    app.agent_running = false;
                    drain_events(app);
                    app.messages.clear();
                }
                Err(e) => {
                    app.agent_running = false;
                    app.messages.push(ChatLine::text(
                        format!("failed to start agent: {}", e),
                        ChatLineKind::Error,
                    ));
                }
            }
        }
    }

    if let Some(rx) = &app.agent_turn_rx {
        if let Ok((agent, result)) = rx.try_recv() {
            app.agent_turn_rx = None;
            app.agent = Some(agent);
            app.agent_running = false;
            drain_events(app);
            persist_streaming_tool(app);
            app.live_assistant.clear();
            app.stream_tool_persisted = false;
            match result {
                Ok(resp) => {
                    let text = sanitize_agent_text(resp.trim());
                    if !text.is_empty() && !is_meta_reply(&text) {
                        app.messages.push(ChatLine::text(text, ChatLineKind::Agent));
                    }
                }
                Err(e) => {
                    app.messages.push(ChatLine::text(
                        format!("error: {}", e),
                        ChatLineKind::Error,
                    ));
                }
            }
            app.scroll_to_bottom();
        }
    }
}

fn handle_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    let ctrl_c = modifiers.contains(KeyModifiers::CONTROL)
        && !modifiers.contains(KeyModifiers::SHIFT)
        && matches!(code, KeyCode::Char('c') | KeyCode::Char('C'));
    if ctrl_c {
        app.should_quit = true;
        return;
    }
    match app.screen {
        Screen::ProviderSelect => handle_provider_key(app, code),
        Screen::ModelSelect => handle_model_key(app, code),
        Screen::Chat => {
            if app.diff_overlay.is_some() {
                handle_diff_key(app, code);
            } else {
                handle_chat_key(app, code, modifiers);
            }
        }
    }
}

fn handle_provider_key(app: &mut App, code: KeyCode) {
    let count = provider_entries().len();
    match code {
        KeyCode::Up | KeyCode::Char('k') => {
            if app.provider_cursor > 0 {
                app.provider_cursor -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.provider_cursor + 1 < count {
                app.provider_cursor += 1;
            }
        }
        KeyCode::Enter => {
            let entries = provider_entries();
            let (kind, _, url) = &entries[app.provider_cursor];
            app.config.provider = kind.clone();
            app.config.base_url = url.to_string();
            app.model_loading = true;
            app.model_error = None;
            app.models.clear();
            app.model_cursor = 0;
            app.model_list_state = ListState::default();
            let pc = app.config.provider_config();
            app.model_thread_rx = Some(spawn_model_fetch(pc));
            app.screen = Screen::ModelSelect;
        }
        KeyCode::Esc => app.should_quit = true,
        _ => {}
    }
}

fn handle_model_key(app: &mut App, code: KeyCode) {
    if app.model_loading {
        if code == KeyCode::Esc {
            app.model_thread_rx = None;
            app.model_loading = false;
            app.screen = Screen::ProviderSelect;
        }
        return;
    }
    match code {
        KeyCode::Up | KeyCode::Char('k') => {
            if app.model_cursor > 0 {
                app.model_cursor -= 1;
                app.model_list_state.select(Some(app.model_cursor));
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.model_cursor + 1 < app.models.len() {
                app.model_cursor += 1;
                app.model_list_state.select(Some(app.model_cursor));
            }
        }
        KeyCode::Enter => {
            if app.models.is_empty() {
                return;
            }
            let model_name = app.models[app.model_cursor].name.clone();
            app.config.model = model_name;

            let (tx, rx) = unbounded::<AgentEvent>();
            app.event_tx_agent = Some(tx.clone());
            app.event_rx = Some(rx);

            let cfg = app.config.clone();
            app.agent_build_rx = Some(spawn_agent_build(cfg, tx));
            app.agent_running = true;
            app.messages.clear();
            app.screen = Screen::Chat;
        }
        KeyCode::Esc => {
            app.screen = Screen::ProviderSelect;
        }
        _ => {}
    }
}

fn line_plain(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect()
}

fn transcript_plain(app: &App) -> String {
    let mut out = String::new();
    for msg in &app.messages {
        match &msg.content {
            ChatContent::Text(t) => {
                let text = if msg.kind == ChatLineKind::Agent {
                    sanitize_agent_text(t)
                } else {
                    t.clone()
                };
                if text.is_empty() {
                    continue;
                }
                out.push_str(&text);
                out.push('\n');
            }
            ChatContent::Rich(lines) => {
                for line in lines {
                    out.push_str(&line_plain(line));
                    out.push('\n');
                }
            }
        }
    }
    if !app.live_assistant.is_empty() {
        out.push_str(&app.live_assistant);
        out.push('\n');
    }
    out
}

fn set_clipboard(text: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut child = Command::new("pbcopy")
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("pbcopy: {}", e))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| anyhow::anyhow!("pbcopy stdin: {}", e))?;
        }
        child
            .wait()
            .map_err(|e| anyhow::anyhow!("pbcopy wait: {}", e))?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        use std::process::{Command, Stdio};
        for cmd in ["wl-copy", "xclip -selection clipboard"] {
            let mut parts = cmd.split_whitespace();
            let prog = parts.next().unwrap();
            let args: Vec<&str> = parts.collect();
            if let Ok(mut child) = Command::new(prog).args(&args).stdin(Stdio::piped()).spawn() {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                if child.wait().map(|s| s.success()).unwrap_or(false) {
                    return Ok(());
                }
            }
        }
        return Err(anyhow::anyhow!("install wl-copy or xclip"));
    }
    #[cfg(target_os = "windows")]
    {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut child = Command::new("clip")
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("clip: {}", e))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| anyhow::anyhow!("clip stdin: {}", e))?;
        }
        child
            .wait()
            .map_err(|e| anyhow::anyhow!("clip wait: {}", e))?;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = text;
        Err(anyhow::anyhow!("clipboard not supported on this platform"))
    }
}

fn copy_transcript(app: &mut App) {
    let text = transcript_plain(app);
    if text.trim().is_empty() {
        app.messages
            .push(ChatLine::text("nothing to copy", ChatLineKind::System));
        return;
    }
    let lines = text.lines().count();
    match set_clipboard(&text) {
        Ok(()) => {
            app.messages.push(ChatLine::text(
                format!("copied {} lines to clipboard", lines),
                ChatLineKind::System,
            ));
        }
        Err(e) => {
            let path = app.config.repo_root.join(".shrimp").join("transcript.txt");
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::write(&path, &text).is_ok() {
                app.messages.push(ChatLine::text(
                    format!("saved to {} ({})", path.display(), e),
                    ChatLineKind::System,
                ));
            } else {
                app.messages.push(ChatLine::text(
                    format!("copy failed: {}", e),
                    ChatLineKind::Error,
                ));
            }
        }
    }
    app.scroll_to_bottom();
}

fn copy_shortcut(modifiers: KeyModifiers, code: KeyCode) -> bool {
    if code != KeyCode::Char('c') && code != KeyCode::Char('C') {
        return false;
    }
    modifiers.contains(KeyModifiers::SUPER)
        || modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
}

fn handle_chat_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    if copy_shortcut(modifiers, code) {
        copy_transcript(app);
        return;
    }

    match code {
        KeyCode::PageUp => {
            app.scroll_up(app.view_height.max(1));
            return;
        }
        KeyCode::PageDown => {
            app.scroll_down(app.view_height.max(1));
            return;
        }
        KeyCode::Up => {
            app.scroll_up(1);
            return;
        }
        KeyCode::Down => {
            app.scroll_down(1);
            return;
        }
        KeyCode::End => {
            app.scroll_to_bottom();
            return;
        }
        _ => {}
    }

    if app.agent_running {
        return;
    }
    match code {
        KeyCode::Esc => app.should_quit = true,
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Enter => {
            let raw = app.input.trim().to_string();
            if raw.is_empty() {
                return;
            }
            app.input.clear();

            if raw.starts_with('/') {
                handle_slash(app, &raw);
                return;
            }

            if app.agent.is_none() {
                return;
            }

            app.messages.push(ChatLine::text(
                format!("› {}", raw),
                ChatLineKind::User,
            ));
            app.scroll_offset = usize::MAX;
            app.agent_running = true;

            let agent = app.agent.take().unwrap();
            app.agent_turn_rx = Some(spawn_agent_turn(agent, raw));
        }
        KeyCode::Char(c) => {
            app.input.push(c);
        }
        _ => {}
    }
}

fn handle_slash(app: &mut App, cmd: &str) {
    match cmd {
        "/help" => {
            app.messages.push(ChatLine::text(
                "commands: /help /copy /model /provider /clear /reindex /undo /quit · ⌘C or Ctrl+Shift+C copies transcript",
                ChatLineKind::System,
            ));
        }
        "/copy" => copy_transcript(app),
        "/quit" => app.should_quit = true,
        "/clear" => app.messages.clear(),
        "/model" => app.screen = Screen::ModelSelect,
        "/provider" => app.screen = Screen::ProviderSelect,
        "/reindex" => {
            if let Some(agent) = &mut app.agent {
                match agent.reindex() {
                    Ok(()) => app.messages.push(ChatLine::text("reindexed.", ChatLineKind::System)),
                    Err(e) => app.messages.push(ChatLine::text(
                        format!("reindex: {}", e),
                        ChatLineKind::Error,
                    )),
                }
            }
        }
        "/undo" => {
            if let Some(agent) = &mut app.agent {
                match agent.undo_last_edit() {
                    Ok(()) => app.messages.push(ChatLine::text("undone.", ChatLineKind::System)),
                    Err(e) => app.messages.push(ChatLine::text(
                        format!("undo: {}", e),
                        ChatLineKind::Error,
                    )),
                }
            }
        }
        other => {
            app.messages.push(ChatLine::text(
                format!("unknown command: {}", other),
                ChatLineKind::Error,
            ));
        }
    }
}

fn handle_diff_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('y') | KeyCode::Enter => {
            app.diff_overlay = None;
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            app.diff_overlay = None;
            if let Some(agent) = &mut app.agent {
                let _ = agent.undo_last_edit();
            }
        }
        _ => {}
    }
}

fn arg_str<'a>(args: &'a Value, keys: &[&str]) -> Option<&'a str> {
    for k in keys {
        if let Some(v) = args.get(*k).and_then(|v| v.as_str()) {
            return Some(v);
        }
    }
    None
}

fn lang_from_path(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "rs" => "rust",
        "py" => "python",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" | "tsx" => "typescript",
        "json" => "json",
        "md" => "markdown",
        "sh" | "bash" => "shell",
        "toml" => "toml",
        "go" => "go",
        _ => "text",
    }
}

fn tool_label(tool: &str, detail: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("⚙ ", Style::default().fg(Color::Cyan)),
        Span::styled(
            tool.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {detail}"), Style::default().fg(Color::Yellow)),
    ])
}

fn looks_like_tool_json(s: &str) -> bool {
    s.contains("```json")
        || s.contains("\"name\":\"write_file\"")
        || s.contains("\"name\": \"write_file\"")
        || s.contains("\"name\":\"patch\"")
        || s.contains("\"name\":\"read_file\"")
        || s.contains("\"name\":\"bash\"")
}

fn extract_tool_json_blob(raw: &str) -> Option<String> {
    if let Some(idx) = raw.find("```json") {
        let after = &raw[idx + 7..];
        let body = after.strip_prefix('\n').unwrap_or(after);
        if let Some(end) = body.find("```") {
            return Some(body[..end].trim().to_string());
        }
        return Some(body.trim().to_string());
    }
    if let Some(idx) = raw.find("{\"name\"") {
        return Some(raw[idx..].trim().to_string());
    }
    if let Some(idx) = raw.find("{ \"name\"") {
        return Some(raw[idx..].trim().to_string());
    }
    None
}

fn extract_partial_string_field(raw: &str, key: &str) -> Option<String> {
    let patterns = [
        format!("\"{key}\":\""),
        format!("\"{key}\": \""),
    ];
    let mut start = None;
    for pat in &patterns {
        if let Some(i) = raw.find(pat) {
            start = Some(i + pat.len());
            break;
        }
    }
    let start = start?;
    let mut out = String::new();
    let mut chars = raw[start..].chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                match next {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
            }
            continue;
        }
        if c == '"' {
            break;
        }
        out.push(c);
    }
    Some(out)
}

fn is_meta_reply(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    matches!(
        t.as_str(),
        "task complete"
            | "task complete."
            | "done"
            | "done."
            | "completed"
            | "completed."
            | "finished"
            | "finished."
            | "all done"
            | "all done."
    ) || (t.len() < 40
        && (t.contains("task complete") || t.contains("task completed") || t == "ok" || t == "ok."))
}

fn sanitize_agent_text(raw: &str) -> String {
    let mut out = raw.to_string();
    while let Some(start) = out.find("```json") {
        if let Some(rel) = out[start + 7..].find("```") {
            let end = start + 7 + rel + 3;
            out.replace_range(start..end, "");
        } else {
            out.truncate(start);
            break;
        }
    }
    while let Some(start) = out.find("{\"name\"") {
        if let Some(rel) = out[start..].find('}') {
            let end = start + rel + 1;
            out.replace_range(start..end, "");
        } else {
            out.truncate(start);
            break;
        }
    }
    out.trim().to_string()
}

fn preview_streaming_tool(raw: &str) -> Option<Vec<Line<'static>>> {
    if !looks_like_tool_json(raw) {
        return None;
    }
    let blob = extract_tool_json_blob(raw)?;
    let calls = ToolCallParser::parse(&blob);
    if let Some(tc) = calls.first() {
        return Some(format_tool_start(&tc.name, &tc.arguments.to_string()));
    }
    if let Ok(val) = serde_json::from_str::<Value>(&blob) {
        if let Some(name) = val.get("name").and_then(|v| v.as_str()) {
            return Some(format_tool_start(name, &blob));
        }
    }
    let name = if blob.contains("write_file") {
        "write_file"
    } else if blob.contains("patch") {
        "patch"
    } else if blob.contains("read_file") {
        "read_file"
    } else if blob.contains("bash") {
        "bash"
    } else {
        return None;
    };
    let path = extract_partial_string_field(raw, "path")
        .or_else(|| extract_partial_string_field(raw, "file_path"))
        .or_else(|| extract_partial_string_field(raw, "filename"))
        .unwrap_or_else(|| "?".to_string());
    let mut out = vec![tool_label(name, &path)];
    match name {
        "write_file" => {
            let content = extract_partial_string_field(raw, "content").unwrap_or_default();
            let lang = lang_from_path(&path);
            if content.is_empty() {
                out.push(Line::from(Span::styled(
                    "  composing…",
                    Style::default().fg(Color::DarkGray),
                )));
            } else {
                push_code_lines(&mut out, &content, lang, 48);
            }
        }
        "patch" => {
            if let Some(old) = extract_partial_string_field(raw, "old_str") {
                for line in old.lines().take(8) {
                    out.push(Line::from(Span::styled(
                        format!("  - {line}"),
                        Style::default().fg(Color::Red),
                    )));
                }
            }
            if let Some(new) = extract_partial_string_field(raw, "new_str") {
                for line in new.lines().take(8) {
                    out.push(Line::from(Span::styled(
                        format!("  + {line}"),
                        Style::default().fg(Color::Green),
                    )));
                }
            }
        }
        "bash" => {
            if let Some(cmd) = extract_partial_string_field(raw, "command") {
                for line in cmd.lines() {
                    out.push(Line::from(vec![
                        Span::styled("  $ ", Style::default().fg(Color::Yellow)),
                        Span::styled(line.to_string(), Style::default().fg(Color::White)),
                    ]));
                }
            }
        }
        _ => {}
    }
    Some(out)
}

fn push_code_lines(
    out: &mut Vec<Line<'static>>,
    content: &str,
    lang: &str,
    max_lines: usize,
) {
    let highlighted = highlight::code_line_spans(content, lang);
    let total = highlighted.len();
    for (i, code_spans) in highlighted.into_iter().take(max_lines).enumerate() {
        let num = format!("{:>3} ", i + 1);
        let mut spans = vec![
            Span::styled("  ", Style::default()),
            Span::styled(num, Style::default().fg(Color::DarkGray)),
        ];
        spans.extend(code_spans);
        out.push(Line::from(spans));
    }
    if total > max_lines {
        out.push(Line::from(Span::styled(
            format!("      … {} more lines", total - max_lines),
            Style::default().fg(Color::DarkGray),
        )));
    }
}

fn format_tool_start(name: &str, args: &str) -> Vec<Line<'static>> {
    let parsed: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    let mut out = Vec::new();

    match name {
        "write_file" => {
            let path = arg_str(&parsed, &["path", "file_path", "filename", "file"]).unwrap_or("?");
            let content = arg_str(&parsed, &["content", "contents", "text", "body"]).unwrap_or("");
            let lang = lang_from_path(path);
            out.push(tool_label("write_file", path));
            push_code_lines(&mut out, content, lang, 32);
        }
        "patch" => {
            let path = arg_str(&parsed, &["path", "file_path", "filename", "file"]).unwrap_or("?");
            let old = arg_str(&parsed, &["old_str", "old", "search", "original"]).unwrap_or("");
            let new = arg_str(&parsed, &["new_str", "new", "replace", "replacement"]).unwrap_or("");
            out.push(tool_label("patch", path));
            for line in old.lines().take(12) {
                out.push(Line::from(Span::styled(
                    format!("  - {line}"),
                    Style::default().fg(Color::Red),
                )));
            }
            for line in new.lines().take(12) {
                out.push(Line::from(Span::styled(
                    format!("  + {line}"),
                    Style::default().fg(Color::Green),
                )));
            }
        }
        "read_file" => {
            let path = arg_str(&parsed, &["path", "file_path", "filename", "file"]).unwrap_or("?");
            out.push(tool_label("read_file", path));
        }
        "bash" => {
            let cmd = arg_str(&parsed, &["command", "cmd", "script", "shell"]).unwrap_or("?");
            out.push(tool_label("bash", ""));
            for line in cmd.lines() {
                out.push(Line::from(vec![
                    Span::styled("  $ ", Style::default().fg(Color::Yellow)),
                    Span::styled(line.to_string(), Style::default().fg(Color::White)),
                ]));
            }
        }
        "search" | "symbol_lookup" => {
            let q = arg_str(&parsed, &["query", "pattern", "name", "symbol", "search"])
                .unwrap_or("?");
            out.push(tool_label(name, q));
        }
        _ => {
            let summary = if parsed.is_object() {
                parsed
                    .as_object()
                    .map(|o| {
                        o.iter()
                            .take(3)
                            .map(|(k, v)| {
                                let snip: String = match v.as_str() {
                                    Some(s) => s.chars().take(60).collect(),
                                    None => v.to_string().chars().take(60).collect(),
                                };
                                format!("{}={}", k, snip)
                            })
                            .collect::<Vec<_>>()
                            .join("  ")
                    })
                    .unwrap_or_default()
            } else {
                args.chars().take(80).collect()
            };
            out.push(tool_label(name, &summary));
        }
    }
    out
}

fn format_tool_end(
    name: &str,
    output: &str,
    success: bool,
    file_path: Option<&str>,
) -> Vec<Line<'static>> {
    let mark = if success { "✓" } else { "✗" };
    let color = if success { Color::Green } else { Color::Red };
    let mut out = Vec::new();

    if name == "read_file" && success && !output.is_empty() {
        let lang = file_path.map(lang_from_path).unwrap_or("text");
        out.push(tool_label("read_file", file_path.unwrap_or("?")));
        push_code_lines(&mut out, output, lang, 48);
        return out;
    }

    if name == "write_file" && success {
        out.push(Line::from(vec![
            Span::styled(format!("{} ", mark), Style::default().fg(color)),
            Span::styled(
                output.chars().take(80).collect::<String>(),
                Style::default().fg(Color::Green),
            ),
        ]));
        return out;
    }

    if name == "bash" && !output.trim().is_empty() {
        out.push(Line::from(vec![
            Span::styled(format!("{} bash", mark), Style::default().fg(color)),
        ]));
        for line in output.lines().take(16) {
            out.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    line.to_string(),
                    Style::default().fg(if success { Color::Gray } else { Color::Red }),
                ),
            ]));
        }
        return out;
    }

    let snip: String = output.chars().take(240).collect();
    out.push(Line::from(vec![
        Span::styled(format!("{} ", mark), Style::default().fg(color)),
        Span::styled(name.to_string(), Style::default().fg(color)),
        Span::styled(format!("  {}", snip), Style::default().fg(Color::Gray)),
    ]));
    out
}

fn persist_streaming_tool(app: &mut App) {
    if app.live_assistant.is_empty() {
        return;
    }
    let lines = preview_streaming_tool(&app.live_assistant).or_else(|| {
        let blob = extract_tool_json_blob(&app.live_assistant)?;
        let calls = ToolCallParser::parse(&blob);
        let tc = calls.first()?;
        Some(format_tool_start(&tc.name, &tc.arguments.to_string()))
    });
    if let Some(lines) = lines {
        app.messages.push(ChatLine::rich(lines, ChatLineKind::Tool));
        app.stream_tool_persisted = true;
    }
}

fn upsert_tool_message(app: &mut App, lines: Vec<Line<'static>>) {
    if app.stream_tool_persisted {
        if let Some(last) = app
            .messages
            .iter_mut()
            .rev()
            .find(|m| m.kind == ChatLineKind::Tool)
        {
            last.content = ChatContent::Rich(lines);
        } else {
            app.messages.push(ChatLine::rich(lines, ChatLineKind::Tool));
        }
        app.stream_tool_persisted = false;
    } else {
        app.messages.push(ChatLine::rich(lines, ChatLineKind::Tool));
    }
}

fn drain_events(app: &mut App) {
    let Some(rx) = &app.event_rx else { return };
    let mut batch = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        batch.push(ev);
    }
    for ev in batch {
        match ev {
            AgentEvent::AssistantDelta { text } => {
                app.live_assistant.push_str(&text);
            }
            AgentEvent::AssistantClear => {
                persist_streaming_tool(app);
                app.live_assistant.clear();
            }
            AgentEvent::ToolStart { name, args } => {
                if let Ok(parsed) = serde_json::from_str::<Value>(&args) {
                    if let Some(path) =
                        arg_str(&parsed, &["path", "file_path", "filename", "file"])
                    {
                        app.last_file_path = Some(path.to_string());
                    }
                }
                if name != "read_file" {
                    let lines = format_tool_start(&name, &args);
                    upsert_tool_message(app, lines);
                }
            }
            AgentEvent::ToolEnd {
                name,
                output,
                success,
                ..
            } => {
                if name == "write_file" || name == "patch" {
                    if success {
                        let path = app.last_file_path.as_deref().unwrap_or("?");
                        app.messages.push(ChatLine::rich(
                            vec![Line::from(vec![
                                Span::styled("✓ ", Style::default().fg(Color::Green)),
                                Span::styled(
                                    format!("saved {path}"),
                                    Style::default().fg(Color::Green),
                                ),
                            ])],
                            ChatLineKind::ToolOk,
                        ));
                    } else {
                        let path = app.last_file_path.as_deref();
                        let lines = format_tool_end(&name, &output, success, path);
                        app.messages.push(ChatLine::rich(lines, ChatLineKind::ToolErr));
                    }
                    continue;
                }
                let kind = if success {
                    ChatLineKind::ToolOk
                } else {
                    ChatLineKind::ToolErr
                };
                let path = app.last_file_path.as_deref();
                let lines = format_tool_end(&name, &output, success, path);
                app.messages.push(ChatLine::rich(lines, kind));
            }
            AgentEvent::TokenUpdate {
                prompt,
                completion,
                budget_pct,
            } => {
                app.status_tokens = format!("prompt:{} compl:{}", prompt, completion);
                app.status_ctx_pct = budget_pct;
            }
            AgentEvent::DiffPreview {
                path,
                old_content,
                new_content,
            } => {
                app.diff_overlay = Some(DiffOverlay {
                    path,
                    old_content,
                    new_content,
                });
            }
            AgentEvent::IndexReady {
                symbols,
                files,
                duration_ms,
            } => {
                app.status_index = format!("{} sym · {} files · {}ms", symbols, files, duration_ms);
            }
            AgentEvent::Error { message } => {
                app.messages.push(ChatLine::text(message, ChatLineKind::Error));
            }
            AgentEvent::TurnComplete { .. } => {}
            _ => {}
        }
    }
}

fn spinner(tick: usize) -> &'static str {
    let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    frames[tick % frames.len()]
}

fn render(f: &mut Frame, app: &mut App) {
    match app.screen {
        Screen::ProviderSelect => render_provider(f, app),
        Screen::ModelSelect => render_model(f, app),
        Screen::Chat => {
            render_chat(f, app);
            if app.diff_overlay.is_some() {
                render_diff(f, app);
            }
        }
    }
}

fn render_provider(f: &mut Frame, app: &App) {
    let area = f.area();
    let block = Block::default()
        .title(" shrimp — choose provider ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let entries = provider_entries();
    let mut lines: Vec<Line> = vec![Line::from(""), Line::from("  Select a provider:")];

    for (i, (_, name, url)) in entries.iter().enumerate() {
        let cursor = if i == app.provider_cursor { "> " } else { "  " };
        let style = if i == app.provider_cursor {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {}{:<14}", cursor, name), style),
            Span::styled(url.to_string(), Style::default().fg(Color::DarkGray)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  ↑/↓ or j/k  ·  Enter to select  ·  Esc to quit",
        Style::default().fg(Color::DarkGray),
    )));

    f.render_widget(Paragraph::new(lines), inner);
}

fn render_model(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let block = Block::default()
        .title(" shrimp — choose model ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.model_loading {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!(
                    "  {} fetching models from {} …",
                    spinner(app.spinner_tick),
                    app.config.provider.as_str()
                ),
                Style::default().fg(Color::Yellow),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Esc — cancel",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        f.render_widget(Paragraph::new(lines), inner);
        return;
    }

    if let Some(err) = &app.model_error {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  Error:",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("  {}", err),
                Style::default().fg(Color::Yellow),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Esc — back to provider select",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        f.render_widget(Paragraph::new(lines), inner);
        return;
    }

    if app.models.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                "  No models.",
                Style::default().fg(Color::DarkGray),
            )),
            inner,
        );
        return;
    }

    let items: Vec<ListItem> = app
        .models
        .iter()
        .map(|m| {
            let size = m
                .size_bytes
                .map(|b| format!("  {:.1} GB", b as f64 / 1_073_741_824.0))
                .unwrap_or_default();
            ListItem::new(format!("{}{}", m.name, size))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Fill(1), Constraint::Length(1)])
        .split(inner);

    f.render_stateful_widget(list, chunks[0], &mut app.model_list_state);

    f.render_widget(
        Paragraph::new(Span::styled(
            "  ↑/↓ select  ·  Enter confirm  ·  Esc back",
            Style::default().fg(Color::DarkGray),
        )),
        chunks[1],
    );
}

fn render_chat(f: &mut Frame, app: &mut App) {
    let area = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(3),
        ])
        .split(area);

    let hints = if app.status_tokens.is_empty() {
        "↑↓ scroll · ⌘C copy · /copy · /help"
    } else {
        "↑↓ scroll · ⌘C copy · /help"
    };
    let status_text = format!(
        " shrimp  {}  {}  {}  ·  {}",
        app.config.provider.as_str(),
        app.config.model,
        app.status_index,
        hints,
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            status_text,
            Style::default().fg(Color::DarkGray),
        )),
        chunks[0],
    );

    let convo = chunks[1];
    let width = convo.width.max(1) as usize;
    let height = convo.height.max(1) as usize;
    let rows = build_wrapped_rows(app, width);

    app.view_rows = rows.len();
    app.view_height = height;

    let offset = app.effective_offset();

    f.render_widget(
        Paragraph::new(rows).scroll((offset as u16, 0)),
        convo,
    );

    let (input_title, border_style) = if app.agent_running {
        (
            format!(" {} streaming ", spinner(app.spinner_tick)),
            Style::default().fg(Color::Magenta),
        )
    } else if app.agent.is_none() && app.agent_build_rx.is_some() {
        (
            format!(" {} indexing ", spinner(app.spinner_tick)),
            Style::default().fg(Color::Yellow),
        )
    } else if !app.status_tokens.is_empty() {
        (
            format!(
                " input  {}  ctx {:.0}% ",
                app.status_tokens, app.status_ctx_pct
            ),
            Style::default().fg(Color::Cyan),
        )
    } else {
        (" input ".to_string(), Style::default().fg(Color::Cyan))
    };

    let cursor = if app.agent_running || app.agent_build_rx.is_some() {
        ""
    } else {
        "▌"
    };

    f.render_widget(
        Paragraph::new(format!(" {}{}", app.input, cursor)).block(
            Block::default()
                .title(input_title)
                .borders(Borders::ALL)
                .border_style(border_style),
        ),
        chunks[2],
    );
}

fn kind_style(kind: &ChatLineKind) -> Style {
    match kind {
        ChatLineKind::User => Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
        ChatLineKind::Agent => Style::default().fg(Color::Gray),
        ChatLineKind::Tool => Style::default().fg(Color::Cyan),
        ChatLineKind::ToolOk => Style::default().fg(Color::Green),
        ChatLineKind::ToolErr => Style::default().fg(Color::Red),
        ChatLineKind::System => Style::default().fg(Color::DarkGray),
        ChatLineKind::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for logical in text.split('\n') {
        if logical.is_empty() {
            rows.push(String::new());
            continue;
        }
        let mut current = String::new();
        let mut current_len = 0usize;
        for word in logical.split_inclusive(' ') {
            let wlen = word.chars().count();
            if current_len + wlen > width && current_len > 0 {
                rows.push(std::mem::take(&mut current));
                current_len = 0;
            }
            if wlen > width {
                if current_len > 0 {
                    rows.push(std::mem::take(&mut current));
                    current_len = 0;
                }
                let mut chunk = String::new();
                for ch in word.chars() {
                    if chunk.chars().count() == width {
                        rows.push(std::mem::take(&mut chunk));
                    }
                    chunk.push(ch);
                }
                if !chunk.is_empty() {
                    current = chunk;
                    current_len = current.chars().count();
                }
            } else {
                current.push_str(word);
                current_len += wlen;
            }
        }
        rows.push(current);
    }
    rows
}

fn push_wrapped(out: &mut Vec<Line<'static>>, text: &str, style: Style, width: usize) {
    for row in wrap_text(text, width) {
        out.push(Line::from(Span::styled(row, style)));
    }
}

fn push_markdown(out: &mut Vec<Line<'static>>, text: &str, style: Style, width: usize) {
    out.extend(markdown::render(text, style, width));
}

fn push_markdown_with_cursor(
    out: &mut Vec<Line<'static>>,
    text: &str,
    style: Style,
    width: usize,
    cursor: &str,
) {
    let mut lines = markdown::render(text, style, width);
    if cursor.is_empty() {
        out.extend(lines);
        return;
    }
    if let Some(last) = lines.last_mut() {
        last.spans.push(Span::raw(cursor.to_string()));
        out.extend(lines);
    } else {
        out.push(Line::from(Span::raw(cursor.to_string())));
    }
}

fn is_card_line(line: &Line<'_>) -> bool {
    let plain: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    plain.starts_with('│')
        || plain.starts_with('╭')
        || plain.starts_with('╰')
        || plain.starts_with('┌')
        || plain.starts_with('└')
}

fn wrap_rich_line(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    let plain: String = line
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    let w = width.saturating_sub(2);
    if plain.chars().count() <= w {
        return vec![line.clone()];
    }
    if is_card_line(line) {
        let truncated: String = plain.chars().take(w.saturating_sub(1)).collect();
        return vec![Line::from(Span::styled(
            format!("{truncated}…"),
            Style::default().fg(Color::DarkGray),
        ))];
    }
    wrap_text(&plain, w)
        .into_iter()
        .map(|row| Line::from(Span::raw(row)))
        .collect()
}

fn push_live_stream(out: &mut Vec<Line<'static>>, app: &App, width: usize) {
    if app.live_assistant.is_empty() {
        return;
    }
    if let Some(lines) = preview_streaming_tool(&app.live_assistant) {
        for line in &lines {
            out.extend(wrap_rich_line(line, width));
        }
        if app.agent_running {
            out.push(Line::from(Span::styled(
                format!("{} streaming", spinner(app.spinner_tick)),
                Style::default().fg(Color::Magenta),
            )));
        }
        return;
    }
    let clean = sanitize_agent_text(&app.live_assistant);
    if clean.is_empty() && looks_like_tool_json(&app.live_assistant) {
        out.push(Line::from(Span::styled(
            format!("{} preparing tool…", spinner(app.spinner_tick)),
            Style::default().fg(Color::Cyan),
        )));
        return;
    }
    if !clean.is_empty() && !is_meta_reply(&clean) {
        let cursor = if app.agent_running { "▌" } else { "" };
        push_markdown_with_cursor(
            out,
            &clean,
            kind_style(&ChatLineKind::Agent),
            width,
            cursor,
        );
    }
}

fn build_wrapped_rows(app: &App, width: usize) -> Vec<Line<'static>> {
    let inner_w = width.saturating_sub(1);
    let mut out = Vec::new();
    for msg in &app.messages {
        match &msg.content {
            ChatContent::Text(text) => {
                let display = if msg.kind == ChatLineKind::Agent {
                    sanitize_agent_text(text)
                } else {
                    text.clone()
                };
                if display.is_empty() || (msg.kind == ChatLineKind::Agent && is_meta_reply(&display))
                {
                    continue;
                }
                let style = kind_style(&msg.kind);
                if msg.kind == ChatLineKind::Agent || msg.kind == ChatLineKind::User {
                    push_markdown(&mut out, &display, style, inner_w);
                } else {
                    push_wrapped(&mut out, &display, style, inner_w);
                }
            }
            ChatContent::Rich(lines) => {
                for line in lines {
                    out.extend(wrap_rich_line(line, inner_w));
                }
            }
        }
    }
    push_live_stream(&mut out, app, inner_w);
    out
}

fn render_diff(f: &mut Frame, app: &App) {
    let Some(diff) = &app.diff_overlay else {
        return;
    };
    let area = centered_rect(70, 60, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" Diff: {} ", diff.path))
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Yellow));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    for line in diff.old_content.lines() {
        lines.push(Line::from(Span::styled(
            format!("- {}", line),
            Style::default().fg(Color::Red),
        )));
    }
    for line in diff.new_content.lines() {
        lines.push(Line::from(Span::styled(
            format!("+ {}", line),
            Style::default().fg(Color::Green),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  [y/Enter] keep   [n/Esc] revert",
        Style::default().fg(Color::DarkGray),
    )));

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn provider_error_hint(kind: &ProviderKind, url: &str, _err: &str) -> String {
    match kind {
        ProviderKind::Ollama => {
            format!(
                "Cannot reach Ollama at {} — start it with: ollama serve",
                url
            )
        }
        ProviderKind::LmStudio => format!(
            "Cannot reach LM Studio at {} — open LM Studio and enable the local server",
            url
        ),
    }
}

fn no_models_hint(kind: &ProviderKind) -> String {
    match kind {
        ProviderKind::Ollama => "No models found — run: ollama pull qwen2.5-coder:7b".into(),
        ProviderKind::LmStudio => "No models loaded — load a model in LM Studio first".into(),
    }
}
