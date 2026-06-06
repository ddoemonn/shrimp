use serde_json::Value;

#[derive(Debug, Clone)]
pub struct ParsedToolCall {
    pub name: String,
    pub arguments: Value,
}

pub struct ToolCallParser;

impl ToolCallParser {
    pub fn parse(content: &str) -> Vec<ParsedToolCall> {
        let mut results = Vec::new();
        Self::parse_fenced(content, &mut results);
        Self::parse_xml_style(content, &mut results);
        if results.is_empty() {
            Self::parse_inline(content, &mut results);
        }
        results
    }

    fn extract_tool_call(obj: &Value) -> Option<ParsedToolCall> {
        if let Some(func) = obj.get("function").filter(|f| f.is_object()) {
            if let Some(tc) = Self::extract_tool_call(func) {
                return Some(tc);
            }
        }

        let name = obj
            .get("name")
            .or_else(|| obj.get("tool"))
            .or_else(|| obj.get("tool_name"))
            .or_else(|| obj.get("function_name"))
            .or_else(|| obj.get("recipient_name"))?
            .as_str()?
            .trim()
            .trim_start_matches("functions.")
            .to_string();
        if name.is_empty() {
            return None;
        }

        let raw_args = obj
            .get("arguments")
            .or_else(|| obj.get("args"))
            .or_else(|| obj.get("parameters"))
            .or_else(|| obj.get("params"))
            .or_else(|| obj.get("input"))
            .cloned();

        let arguments = match raw_args {
            Some(Value::String(s)) => serde_json::from_str::<Value>(s.trim())
                .unwrap_or(Value::Object(serde_json::Map::new())),
            Some(v @ Value::Object(_)) => v,
            _ => Value::Object(serde_json::Map::new()),
        };
        Some(ParsedToolCall { name, arguments })
    }

    fn parse_fenced(content: &str, out: &mut Vec<ParsedToolCall>) {
        let marker = "```json";
        let close = "```";
        let mut search_from = 0;
        while search_from < content.len() {
            let remaining = &content[search_from..];
            let Some(open_pos) = remaining.find(marker) else {
                break;
            };
            let after_marker = search_from + open_pos + marker.len();
            let json_start = if content.as_bytes().get(after_marker) == Some(&b'\n') {
                after_marker + 1
            } else {
                after_marker
            };
            if json_start > content.len() {
                break;
            }
            let Some(close_pos) = content[json_start..].find(close) else {
                break;
            };
            let json_str = content[json_start..json_start + close_pos].trim();
            if let Ok(val) = serde_json::from_str::<Value>(json_str) {
                if let Some(tc) = Self::extract_tool_call(&val) {
                    out.push(tc);
                }
            }
            search_from = json_start + close_pos + close.len();
        }
    }

    fn parse_xml_style(content: &str, out: &mut Vec<ParsedToolCall>) {
        let open_tag = "<tool_call>";
        let close_tag = "</tool_call>";
        let mut search_from = 0;
        while search_from < content.len() {
            let remaining = &content[search_from..];
            let Some(open_pos) = remaining.find(open_tag) else {
                break;
            };
            let json_start = search_from + open_pos + open_tag.len();
            if json_start > content.len() {
                break;
            }
            let Some(close_pos) = content[json_start..].find(close_tag) else {
                break;
            };
            let json_str = content[json_start..json_start + close_pos].trim();
            if let Ok(val) = serde_json::from_str::<Value>(json_str) {
                if let Some(tc) = Self::extract_tool_call(&val) {
                    out.push(tc);
                }
            }
            search_from = json_start + close_pos + close_tag.len();
        }
    }

    fn parse_inline(content: &str, out: &mut Vec<ParsedToolCall>) {
        let bytes = content.as_bytes();
        let len = bytes.len();
        let mut i = 0;
        while i < len {
            if bytes[i] == b'{' {
                let start = i;
                let mut depth: i32 = 0;
                let mut in_str = false;
                let mut escape = false;
                let mut j = i;
                loop {
                    if j >= len {
                        break;
                    }
                    let c = bytes[j];
                    if escape {
                        escape = false;
                    } else if in_str {
                        if c == b'\\' {
                            escape = true;
                        } else if c == b'"' {
                            in_str = false;
                        }
                    } else if c == b'"' {
                        in_str = true;
                    } else if c == b'{' {
                        depth += 1;
                    } else if c == b'}' {
                        depth -= 1;
                        if depth == 0 {
                            if let Ok(val) = serde_json::from_str::<Value>(&content[start..=j]) {
                                if let Some(tc) = Self::extract_tool_call(&val) {
                                    out.push(tc);
                                }
                            }
                            i = j;
                            break;
                        }
                    }
                    j += 1;
                }
            }
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fenced_json() {
        let content = "Here is the tool:\n```json\n{\"name\":\"write_file\",\"arguments\":{\"path\":\"hello.py\",\"content\":\"print(1)\"}}\n```";
        let calls = ToolCallParser::parse(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
    }

    #[test]
    fn test_xml_style() {
        let content =
            "<tool_call>{\"name\":\"read_file\",\"arguments\":{\"path\":\"foo.rs\"}}</tool_call>";
        let calls = ToolCallParser::parse(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
    }

    #[test]
    fn test_inline_json() {
        let content = "Sure! {\"name\":\"bash\",\"arguments\":{\"command\":\"ls\"}}";
        let calls = ToolCallParser::parse(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
    }

    #[test]
    fn test_alternate_keys() {
        let content = "```json\n{\"tool\":\"search\",\"args\":{\"query\":\"main\"}}\n```";
        let calls = ToolCallParser::parse(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "search");
    }

    #[test]
    fn test_string_encoded_arguments() {
        let content = "```json\n{\"name\":\"write_file\",\"arguments\":\"{\\\"path\\\":\\\"a.py\\\",\\\"content\\\":\\\"print(1)\\\"}\"}\n```";
        let calls = ToolCallParser::parse(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert_eq!(calls[0].arguments.get("path").unwrap(), "a.py");
    }

    #[test]
    fn test_openai_function_wrapper() {
        let content = "```json\n{\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":{\"path\":\"x.rs\"}}}\n```";
        let calls = ToolCallParser::parse(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments.get("path").unwrap(), "x.rs");
    }

    #[test]
    fn test_function_name_prefix_stripped() {
        let content = "{\"tool_name\":\"functions.bash\",\"parameters\":{\"command\":\"ls\"}}";
        let calls = ToolCallParser::parse(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
    }
}
