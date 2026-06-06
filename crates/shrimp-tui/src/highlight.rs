use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::LazyLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "boolean",
    "comment",
    "comment.documentation",
    "constant",
    "constant.builtin",
    "constructor",
    "constructor.builtin",
    "embedded",
    "function",
    "function.builtin",
    "function.method",
    "keyword",
    "module",
    "number",
    "operator",
    "property",
    "property.builtin",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.escape",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.member",
    "variable.parameter",
];

fn style_for_capture(name: &str) -> Style {
    match name {
        "keyword" => Style::default()
            .fg(Color::Rgb(198, 120, 221))
            .add_modifier(Modifier::BOLD),
        "string" | "string.escape" | "string.special" => {
            Style::default().fg(Color::Rgb(152, 195, 151))
        }
        "comment" | "comment.documentation" => Style::default()
            .fg(Color::Rgb(92, 99, 112))
            .add_modifier(Modifier::ITALIC),
        "function" | "function.builtin" | "function.method" => {
            Style::default().fg(Color::Rgb(229, 192, 123))
        }
        "type" | "type.builtin" | "constructor" | "constructor.builtin" => {
            Style::default().fg(Color::Rgb(97, 175, 239))
        }
        "number" | "constant" | "constant.builtin" | "boolean" => {
            Style::default().fg(Color::Rgb(209, 154, 102))
        }
        "variable" | "variable.parameter" | "variable.member" | "property"
        | "property.builtin" => Style::default().fg(Color::Rgb(224, 108, 117)),
        "operator" | "punctuation" | "punctuation.bracket" | "punctuation.delimiter"
        | "punctuation.special" => Style::default().fg(Color::Rgb(171, 178, 191)),
        "tag" | "module" => Style::default().fg(Color::Rgb(224, 108, 117)),
        _ => Style::default().fg(Color::Rgb(171, 178, 191)),
    }
}

fn make_config(
    language: tree_sitter::Language,
    name: &str,
    highlights: &str,
    injections: &str,
    locals: &str,
) -> Option<HighlightConfiguration> {
    let mut config =
        HighlightConfiguration::new(language, name, highlights, injections, locals).ok()?;
    config.configure(HIGHLIGHT_NAMES);
    Some(config)
}

fn init_configs() -> HashMap<&'static str, HighlightConfiguration> {
    let mut map = HashMap::new();
    if let Some(c) = make_config(
        tree_sitter_rust::LANGUAGE.into(),
        "rust",
        tree_sitter_rust::HIGHLIGHTS_QUERY,
        tree_sitter_rust::INJECTIONS_QUERY,
        "",
    ) {
        map.insert("rust", c);
    }
    if let Some(c) = make_config(
        tree_sitter_python::LANGUAGE.into(),
        "python",
        tree_sitter_python::HIGHLIGHTS_QUERY,
        "",
        "",
    ) {
        map.insert("python", c);
    }
    if let Some(c) = make_config(
        tree_sitter_javascript::LANGUAGE.into(),
        "javascript",
        tree_sitter_javascript::HIGHLIGHT_QUERY,
        tree_sitter_javascript::INJECTIONS_QUERY,
        tree_sitter_javascript::LOCALS_QUERY,
    ) {
        map.insert("javascript", c);
    }
    if let Some(c) = make_config(
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "typescript",
        tree_sitter_typescript::HIGHLIGHTS_QUERY,
        "",
        "",
    ) {
        map.insert("typescript", c);
    }
    if let Some(c) = make_config(
        tree_sitter_go::LANGUAGE.into(),
        "go",
        tree_sitter_go::HIGHLIGHTS_QUERY,
        "",
        "",
    ) {
        map.insert("go", c);
    }
    map
}

static CONFIGS: LazyLock<HashMap<&'static str, HighlightConfiguration>> =
    LazyLock::new(init_configs);

thread_local! {
    static HIGHLIGHTER: RefCell<Highlighter> = RefCell::new(Highlighter::new());
}

fn fallback_line(line: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let trimmed = line.trim_start();
    if trimmed.starts_with("//")
        || trimmed.starts_with('#')
        || trimmed.starts_with("--")
    {
        return vec![Span::styled(
            line.to_string(),
            Style::default()
                .fg(Color::Rgb(92, 99, 112))
                .add_modifier(Modifier::ITALIC),
        )];
    }
    let mut i = 0;
    let chars: Vec<char> = line.chars().collect();
    while i < chars.len() {
        let c = chars[i];
        if c == '"' || c == '\'' {
            let quote = c;
            let start = i;
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' {
                    i += 2;
                    continue;
                }
                if chars[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            spans.push(Span::styled(
                chars[start..i].iter().collect::<String>(),
                Style::default().fg(Color::Rgb(152, 195, 151)),
            ));
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            spans.push(Span::styled(
                chars[start..i].iter().collect::<String>(),
                Style::default().fg(Color::Rgb(209, 154, 102)),
            ));
            continue;
        }
        if c.is_alphanumeric() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let kw = matches!(
                word.as_str(),
                "fn" | "let" | "mut" | "use" | "pub" | "struct" | "impl" | "match" | "if" | "else"
                    | "return" | "mod" | "async" | "await" | "for" | "while" | "loop" | "enum"
                    | "trait" | "where" | "def" | "class" | "import" | "from" | "const"
                    | "function" | "var" | "export"
            );
            let style = if kw {
                Style::default()
                    .fg(Color::Rgb(198, 120, 221))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Rgb(171, 178, 191))
            };
            spans.push(Span::styled(word, style));
            continue;
        }
        spans.push(Span::styled(
            c.to_string(),
            Style::default().fg(Color::Rgb(171, 178, 191)),
        ));
        i += 1;
    }
    if spans.is_empty() {
        spans.push(Span::raw(line.to_string()));
    }
    spans
}

fn style_for_active(active: &[usize]) -> Style {
    active
        .last()
        .and_then(|idx| HIGHLIGHT_NAMES.get(*idx))
        .map(|name| style_for_capture(name))
        .unwrap_or_else(|| Style::default().fg(Color::Rgb(171, 178, 191)))
}

fn events_to_line_spans(source: &str, lang: &str) -> Vec<Vec<Span<'static>>> {
    let Some(config) = CONFIGS.get(lang) else {
        return source.lines().map(fallback_line).collect();
    };

    HIGHLIGHTER.with(|cell| {
        let mut highlighter = cell.borrow_mut();
        let Ok(iter) = highlighter.highlight(config, source.as_bytes(), None, |_| None) else {
            return source.lines().map(fallback_line).collect();
        };

        let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
        let mut active: Vec<usize> = Vec::new();
        let mut pending = String::new();
        let mut pending_style = Style::default().fg(Color::Rgb(171, 178, 191));

        let flush_pending = |lines: &mut Vec<Vec<Span<'static>>>, pending: &mut String, style: Style| {
            if pending.is_empty() {
                return;
            }
            let text = std::mem::take(pending);
            if let Some(last) = lines.last_mut() {
                if let Some(prev) = last.last() {
                    if prev.style == style {
                        let merged = format!("{}{}", prev.content, text);
                        last.pop();
                        last.push(Span::styled(merged, style));
                        return;
                    }
                }
                last.push(Span::styled(text, style));
            }
        };

        for event in iter {
            let Ok(event) = event else { break };
            match event {
                HighlightEvent::Source { start, end } => {
                    let chunk = &source[start..end];
                    for ch in chunk.chars() {
                        if ch == '\n' {
                            flush_pending(&mut lines, &mut pending, pending_style);
                            pending_style = style_for_active(&active);
                            lines.push(Vec::new());
                        } else {
                            if pending.is_empty() {
                                pending_style = style_for_active(&active);
                            }
                            pending.push(ch);
                        }
                    }
                }
                HighlightEvent::HighlightStart(h) => {
                    flush_pending(&mut lines, &mut pending, pending_style);
                    active.push(h.0);
                    pending_style = style_for_active(&active);
                }
                HighlightEvent::HighlightEnd => {
                    flush_pending(&mut lines, &mut pending, pending_style);
                    active.pop();
                    pending_style = style_for_active(&active);
                }
            }
        }
        flush_pending(&mut lines, &mut pending, pending_style);
        lines
    })
}

pub fn code_line_spans(content: &str, lang: &str) -> Vec<Vec<Span<'static>>> {
    let lang = match lang {
        "shell" | "bash" | "sh" => "text",
        "json" | "toml" | "markdown" | "text" => lang,
        other if CONFIGS.contains_key(other) => other,
        _ => "text",
    };
    if lang == "text" || !CONFIGS.contains_key(lang) {
        return content.lines().map(fallback_line).collect();
    }
    events_to_line_spans(content, lang)
}
