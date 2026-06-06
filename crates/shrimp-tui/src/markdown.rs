use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::highlight;

fn lang_from_tag(tag: &str) -> &str {
    let t = tag.trim();
    if t.eq_ignore_ascii_case("rust") || t == "rs" {
        "rust"
    } else if t.eq_ignore_ascii_case("python") || t == "py" {
        "python"
    } else if t.eq_ignore_ascii_case("javascript") || t == "js" {
        "javascript"
    } else if t.eq_ignore_ascii_case("typescript") || t == "ts" {
        "typescript"
    } else if t.eq_ignore_ascii_case("go") {
        "go"
    } else if t.is_empty() || matches!(t, "toml" | "json" | "bash" | "sh" | "shell") {
        "text"
    } else {
        t
    }
}

fn merge_spans(spans: Vec<Span<'static>>) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    for span in spans {
        if span.content.is_empty() {
            continue;
        }
        if let Some(last) = out.last_mut() {
            if last.style == span.style {
                let merged = format!("{}{}", last.content, span.content);
                last.content = merged.into();
                continue;
            }
        }
        out.push(span);
    }
    out
}

fn chars_to_line(chars: Vec<(char, Style)>) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut cur_style = Style::default();
    for (ch, style) in chars {
        if buf.is_empty() {
            cur_style = style;
            buf.push(ch);
        } else if style == cur_style {
            buf.push(ch);
        } else {
            spans.push(Span::styled(std::mem::take(&mut buf), cur_style));
            cur_style = style;
            buf.push(ch);
        }
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, cur_style));
    }
    Line::from(merge_spans(spans))
}

pub fn wrap_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Line<'static>> {
    let w = width.max(1);
    let mut flat: Vec<(char, Style)> = Vec::new();
    for span in spans {
        let style = span.style;
        for ch in span.content.chars() {
            flat.push((ch, style));
        }
    }

    let mut lines = Vec::new();
    let mut current: Vec<(char, Style)> = Vec::new();
    let mut len = 0usize;

    for (ch, style) in flat {
        if ch == '\n' {
            lines.push(chars_to_line(std::mem::take(&mut current)));
            len = 0;
            continue;
        }
        if len >= w && !current.is_empty() {
            lines.push(chars_to_line(std::mem::take(&mut current)));
            len = 0;
        }
        current.push((ch, style));
        len += 1;
    }
    if !current.is_empty() {
        lines.push(chars_to_line(current));
    }
    lines
}

fn parse_inline(text: &str, base: Style) -> Vec<Span<'static>> {
    let code_style = Style::default().fg(Color::Rgb(152, 195, 151));
    let bold_style = base.add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;

    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            spans.push(Span::styled(
                chars[i + 1].to_string(),
                base,
            ));
            i += 2;
            continue;
        }
        if chars[i] == '`' {
            let start = i + 1;
            i = start;
            let mut inner = String::new();
            while i < chars.len() && chars[i] != '`' {
                inner.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            spans.push(Span::styled(inner, code_style));
            continue;
        }
        if i + 1 < chars.len() && chars[i] == '*' && chars[i + 1] == '*' {
            let start = i + 2;
            i = start;
            let mut inner = String::new();
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '*') {
                inner.push(chars[i]);
                i += 1;
            }
            if i + 1 < chars.len() {
                i += 2;
            } else {
                i = chars.len();
            }
            spans.push(Span::styled(inner, bold_style));
            continue;
        }
        let start = i;
        while i < chars.len() {
            if chars[i] == '\\' || chars[i] == '`' {
                break;
            }
            if i + 1 < chars.len() && chars[i] == '*' && chars[i + 1] == '*' {
                break;
            }
            i += 1;
        }
        spans.push(Span::styled(
            chars[start..i].iter().collect::<String>(),
            base,
        ));
    }
    merge_spans(spans)
}

fn header_style(level: u8) -> Style {
    let color = match level {
        1 => Color::Rgb(229, 192, 123),
        2 => Color::Rgb(97, 175, 239),
        _ => Color::Rgb(198, 120, 221),
    };
    Style::default()
        .fg(color)
        .add_modifier(Modifier::BOLD)
}

fn parse_header(line: &str) -> Option<(u8, &str)> {
    let mut level = 0u8;
    for ch in line.chars() {
        if ch == '#' {
            level += 1;
        } else {
            break;
        }
    }
    if (1..=6).contains(&level) && line.chars().nth(level as usize) == Some(' ') {
        Some((level, line[level as usize..].trim()))
    } else {
        None
    }
}

fn parse_ordered_item(line: &str) -> Option<(&str, &str)> {
    let t = line.trim_start();
    let dot = t.find('.')?;
    if dot == 0 {
        return None;
    }
    if !t[..dot].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let after_dot = t.get(dot + 1..)?;
    if !after_dot.starts_with(' ') && !after_dot.starts_with('\t') {
        return None;
    }
    let num = &t[..=dot];
    Some((num, after_dot.trim_start()))
}

fn parse_bullet_item(line: &str) -> Option<(usize, &str)> {
    let indent = line.len() - line.trim_start().len();
    let t = line.trim_start();
    let rest = t.strip_prefix("* ").or_else(|| t.strip_prefix("- "))?;
    Some((indent, rest))
}

fn render_code_block(body: &str, lang: &str, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let tag = lang_from_tag(lang);
    let border = Style::default().fg(Color::Rgb(60, 60, 80));
    out.push(Line::from(Span::styled("  ┌─ code ", border)));
    for code_spans in highlight::code_line_spans(body, tag) {
        let mut spans = vec![Span::styled("  │ ", border)];
        spans.extend(code_spans);
        out.extend(wrap_spans(spans, width));
    }
    out.push(Line::from(Span::styled("  └─", border)));
    out
}

pub fn render(text: &str, base: Style, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut lines = text.lines().peekable();
    let bullet_prefix = Style::default().fg(Color::Rgb(97, 175, 239));
    let number_prefix = Style::default().fg(Color::Rgb(209, 154, 102));

    while let Some(line) = lines.next() {
        if line.trim().starts_with("```") {
            let tag = line.trim().trim_start_matches("```").trim();
            let mut body = String::new();
            while let Some(&next) = lines.peek() {
                if next.trim() == "```" {
                    lines.next();
                    break;
                }
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(next);
                lines.next();
            }
            out.extend(render_code_block(&body, tag, width));
            continue;
        }

        if line.trim().is_empty() {
            out.push(Line::from(""));
            continue;
        }

        if let Some((level, title)) = parse_header(line) {
            let spans = parse_inline(title, header_style(level));
            out.extend(wrap_spans(spans, width));
            continue;
        }

        if let Some((num, content)) = parse_ordered_item(line) {
            let mut spans = vec![
                Span::styled(format!("{num} "), number_prefix),
            ];
            spans.extend(parse_inline(content, base));
            out.extend(wrap_spans(spans, width));
            continue;
        }

        if let Some((indent, content)) = parse_bullet_item(line) {
            let pad = " ".repeat(indent + 2);
            let mut spans = vec![
                Span::styled(format!("{pad}• "), bullet_prefix),
            ];
            spans.extend(parse_inline(content, base));
            out.extend(wrap_spans(spans, width));
            continue;
        }

        let spans = parse_inline(line, base);
        out.extend(wrap_spans(spans, width));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_bold_and_code() {
        let lines = render(
            "Use `src/main.rs` and **important**",
            Style::default().fg(Color::Gray),
            80,
        );
        let plain: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(plain.contains("src/main.rs"));
        assert!(plain.contains("important"));
    }

    #[test]
    fn renders_numbered_list() {
        let text = "1. **First** item\n2. Second";
        let lines = render(text, Style::default(), 80);
        assert!(lines.len() >= 2);
    }
}
