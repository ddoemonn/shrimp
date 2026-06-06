use tree_sitter::{Language, Node, Parser};

use crate::{Symbol, SymbolKind};

pub fn language_for_ext(ext: &str) -> Option<Language> {
    match ext {
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "py" => Some(tree_sitter_python::LANGUAGE.into()),
        "js" | "jsx" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "ts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        _ => None,
    }
}

pub fn supported_extension(ext: &str) -> bool {
    language_for_ext(ext).is_some()
}

pub fn parse_file(path: &str, source: &str, ext: &str) -> Vec<Symbol> {
    let language = match language_for_ext(ext) {
        Some(l) => l,
        None => return Vec::new(),
    };
    let mut parser = Parser::new();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let tree = match parser.parse(source.as_bytes(), None) {
        Some(t) => t,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    walk(tree.root_node(), source.as_bytes(), path, &mut out);
    out
}

fn classify(kind: &str) -> Option<SymbolKind> {
    match kind {
        "function_item" | "function_definition" | "function_declaration" => {
            Some(SymbolKind::Function)
        }
        "arrow_function" => Some(SymbolKind::Function),
        "method_definition" | "method_declaration" => Some(SymbolKind::Method),
        "struct_item" => Some(SymbolKind::Struct),
        "class_definition" | "class_declaration" => Some(SymbolKind::Class),
        "trait_item" => Some(SymbolKind::Trait),
        "enum_item" | "enum_declaration" => Some(SymbolKind::Enum),
        "impl_item" => Some(SymbolKind::Impl),
        "mod_item" | "module" => Some(SymbolKind::Module),
        _ => None,
    }
}

fn walk(node: Node, source: &[u8], file: &str, out: &mut Vec<Symbol>) {
    if let Some(sk) = classify(node.kind()) {
        if let Some(name_node) = node.child_by_field_name("name") {
            if let Ok(name) = name_node.utf8_text(source) {
                if !name.is_empty() {
                    let line = node.start_position().row as u32 + 1;
                    let end_line = node.end_position().row as u32 + 1;
                    let signature = node
                        .utf8_text(source)
                        .unwrap_or("")
                        .lines()
                        .next()
                        .unwrap_or("")
                        .to_owned();
                    out.push(Symbol {
                        name: name.to_owned(),
                        kind: sk,
                        file: file.to_owned(),
                        line,
                        end_line,
                        signature,
                    });
                }
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, source, file, out);
    }
}
