//! Tree-sitter based syntax spans cached by the UI at buffer revision boundaries.
use std::borrow::Cow;
use std::path::Path;
use tree_sitter::{Language, Parser};
pub const MAX_SOURCE_BYTES: usize = 256 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Keyword,
    String,
    Comment,
    Number,
    Function,
    Type,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub style: Style,
}
fn language(path: &Path) -> Option<Language> {
    match path.extension()?.to_str()? {
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "ts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "js" | "jsx" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "py" => Some(tree_sitter_python::LANGUAGE.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "cs" => Some(tree_sitter_c_sharp::LANGUAGE.into()),
        "vb" => Some(tree_sitter_vb_dotnet::LANGUAGE.into()),
        "c" | "h" => Some(tree_sitter_c::LANGUAGE.into()),
        "cc" | "cpp" | "cxx" | "hpp" | "hxx" => Some(tree_sitter_cpp::LANGUAGE.into()),
        "json" | "jsonc" => Some(tree_sitter_json::LANGUAGE.into()),
        "yaml" | "yml" => Some(tree_sitter_yaml::LANGUAGE.into()),
        "toml" => Some(tree_sitter_toml_ng::LANGUAGE.into()),
        "html" | "htm" => Some(tree_sitter_html::LANGUAGE.into()),
        "css" => Some(tree_sitter_css::LANGUAGE.into()),
        "sh" | "bash" => Some(tree_sitter_bash::LANGUAGE.into()),
        "lua" => Some(tree_sitter_lua::LANGUAGE.into()),
        "md" | "markdown" => Some(tree_sitter_md::LANGUAGE.into()),
        _ => None,
    }
}
pub fn supported(path: &Path) -> bool {
    language(path).is_some()
}
fn style(kind: &str) -> Option<Style> {
    if kind.contains("comment") {
        Some(Style::Comment)
    } else if kind.contains("string") || kind.contains("scalar") || kind == "code_span" {
        Some(Style::String)
    } else if kind.contains("number") || kind.contains("integer") || kind.contains("float") {
        Some(Style::Number)
    } else if kind.contains("type_identifier")
        || kind.contains("primitive_type")
        || kind == "tag_name"
        || kind == "property_name"
        || kind.contains("heading")
    {
        Some(Style::Type)
    } else if kind == "function_identifier" || kind == "command_name" {
        Some(Style::Function)
    } else if matches!(
        kind,
        "fn" | "let"
            | "mut"
            | "pub"
            | "impl"
            | "struct"
            | "enum"
            | "trait"
            | "use"
            | "mod"
            | "return"
            | "if"
            | "else"
            | "for"
            | "while"
            | "match"
            | "async"
            | "await"
            | "function"
            | "const"
            | "class"
            | "import"
            | "export"
            | "def"
            | "from"
            | "pass"
            | "with"
            | "as"
            | "try"
            | "except"
            | "true"
            | "false"
            | "null"
            | "int"
            | "char"
            | "void"
            | "include"
            | "then"
            | "fi"
            | "do"
            | "done"
            | "package"
            | "func"
            | "defer"
            | "go"
            | "select"
            | "namespace"
            | "using"
            | "public"
            | "private"
            | "static"
            | "new"
            | "Dim"
            | "Sub"
            | "Function"
            | "End"
            | "Module"
            | "Public"
            | "Private"
    ) {
        Some(Style::Keyword)
    } else {
        None
    }
}
pub fn highlight(path: &Path, text: &str) -> Vec<Vec<Span>> {
    let Some(language) = language(path) else {
        return Vec::new();
    };
    if text.len() > MAX_SOURCE_BYTES {
        return Vec::new();
    }
    let mut spans = vec![Vec::new(); text.lines().count().max(1)];
    let mut parser = Parser::new();
    if parser.set_language(&language).is_err() {
        return spans;
    }
    let markdown = matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("md" | "markdown")
    );
    let vb = path.extension().and_then(|ext| ext.to_str()) == Some("vb");
    let source: Cow<'_, str> = if markdown && !text.ends_with('\n') {
        Cow::Owned(format!("{text}\n"))
    } else {
        Cow::Borrowed(text)
    };
    let Some(tree) = parser.parse(source.as_ref(), None) else {
        return spans;
    };
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        let node_style = if vb && node.kind() == "identifier" {
            match node.parent().map(|parent| parent.kind()) {
                Some("module_block" | "class_block") => Some(Style::Type),
                Some("method_declaration") => Some(Style::Function),
                _ => None,
            }
        } else {
            style(node.kind())
        };
        if let Some(style) = node_style {
            let start = node.start_position();
            let end = node.end_position();
            for row in start.row..=end.row.min(spans.len() - 1) {
                let line_start = if row == start.row { start.column } else { 0 };
                let line_end = if row == end.row {
                    end.column
                } else {
                    usize::MAX
                };
                if line_start < line_end {
                    spans[row].push(Span {
                        start: line_start,
                        end: line_end,
                        style,
                    })
                }
            }
            // A styled parent owns its children, so strings and comments do not get keyword colors.
            continue;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child)
        }
    }
    for line in &mut spans {
        line.sort_by_key(|span| span.start)
    }
    spans
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rust_keywords_and_strings_are_parsed() {
        let spans = highlight(Path::new("main.rs"), "fn main() { let x = \"hello\"; }\n");
        assert!(spans[0].iter().any(|span| span.style == Style::Keyword));
        assert!(spans[0].iter().any(|span| span.style == Style::String));
    }
    #[test]
    fn typescript_parser_loads() {
        let spans = highlight(Path::new("main.ts"), "const x = 'hello';\n");
        assert!(spans[0].iter().any(|span| span.style == Style::String));
    }
    #[test]
    fn required_language_grammars_parse() {
        let samples = [
            ("main.rs", "fn main() {}"),
            ("main.ts", "const value = 1;"),
            ("main.tsx", "const View = () => <main>Hello</main>;"),
            ("main.js", "const value = 1;"),
            ("main.py", "def main(): pass"),
            ("main.go", "package main\nfunc main() {}"),
            ("main.cs", "class Program { static void Main() {} }"),
            (
                "main.vb",
                "Module Program\nSub Main()\nEnd Sub\nEnd Module\n",
            ),
            ("main.c", "int main(void) { return 0; }"),
            ("main.cpp", "int main() { return 0; }"),
            ("main.json", "{\"value\": 1}"),
            ("main.yaml", "value: hello"),
            ("main.toml", "value = \"hello\""),
            ("main.html", "<main>Hello</main>"),
            ("main.css", "main { color: red; }"),
            ("main.sh", "echo hello"),
            ("main.lua", "local value = 1"),
            ("main.md", "# Heading\n"),
        ];
        for (name, source) in samples {
            let grammar = language(Path::new(name)).expect(name);
            let mut parser = Parser::new();
            parser.set_language(&grammar).expect(name);
            let tree = parser.parse(source, None).expect(name);
            assert!(
                !tree.root_node().has_error(),
                "{name}: {}",
                tree.root_node().to_sexp()
            );
            assert!(
                highlight(Path::new(name), source)
                    .iter()
                    .any(|line| !line.is_empty()),
                "no syntax spans: {name}: {}",
                tree.root_node().to_sexp()
            );
        }
    }
    #[test]
    fn markdown_heading_without_final_newline_is_highlighted() {
        let spans = highlight(Path::new("note.md"), "# Heading");
        assert!(spans[0].iter().any(|span| span.style == Style::Type));
    }
    #[test]
    fn large_sources_skip_expensive_full_parse() {
        let source = "fn main() {}\n".repeat(MAX_SOURCE_BYTES / 12 + 1);
        assert!(source.len() > MAX_SOURCE_BYTES);
        assert!(highlight(Path::new("large.rs"), &source).is_empty());
        assert!(highlight(Path::new("notes.txt"), "plain text").is_empty());
    }
}
