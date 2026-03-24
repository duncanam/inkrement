use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Convert a Markdown string to Typst markup.
///
/// Handles headings, bold, italic, code spans, code blocks, links, lists,
/// and paragraphs. Unsupported elements pass through as plain text.
pub(crate) fn markdown_to_typst(markdown: &str) -> String {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let parser = Parser::new_ext(markdown, options);

    let mut out = String::new();
    let mut list_stack: Vec<ListKind> = Vec::new();

    for event in parser {
        match event {
            // --- Block-level elements ---
            Event::Start(Tag::Heading { level, .. }) => {
                let prefix = "=".repeat(heading_depth(level));
                out.push_str(&format!("\n{prefix} "));
            }
            Event::End(TagEnd::Heading(_)) => {
                out.push('\n');
            }

            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                out.push_str("\n\n");
            }

            Event::Start(Tag::BlockQuote(_)) => {
                out.push_str("#block(inset: (left: 12pt), stroke: (left: 2pt + gray))[\n");
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                out.push_str("]\n\n");
            }

            Event::Start(Tag::CodeBlock(_kind)) => {
                out.push_str("```\n");
            }
            Event::End(TagEnd::CodeBlock) => {
                out.push_str("```\n\n");
            }

            // --- Lists ---
            Event::Start(Tag::List(first_number)) => {
                let kind = match first_number {
                    Some(start) => ListKind::Ordered(start),
                    None => ListKind::Unordered,
                };
                list_stack.push(kind);
            }
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
                if list_stack.is_empty() {
                    out.push('\n');
                }
            }

            Event::Start(Tag::Item) => {
                let indent = "  ".repeat(list_stack.len().saturating_sub(1));
                match list_stack.last_mut() {
                    Some(ListKind::Ordered(n)) => {
                        out.push_str(&format!("{indent}+ "));
                        *n += 1;
                    }
                    Some(ListKind::Unordered) | None => {
                        out.push_str(&format!("{indent}- "));
                    }
                }
            }
            Event::End(TagEnd::Item) => {
                // Ensure newline after item
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }

            // --- Inline elements ---
            Event::Start(Tag::Strong) => out.push('*'),
            Event::End(TagEnd::Strong) => out.push('*'),

            Event::Start(Tag::Emphasis) => out.push('_'),
            Event::End(TagEnd::Emphasis) => out.push('_'),

            Event::Start(Tag::Strikethrough) => out.push_str("#strike["),
            Event::End(TagEnd::Strikethrough) => out.push(']'),

            Event::Start(Tag::Link { dest_url, .. }) => {
                out.push_str(&format!("#link(\"{dest_url}\")["));
            }
            Event::End(TagEnd::Link) => out.push(']'),

            Event::Code(code) => {
                out.push_str(&format!("`{code}`"));
            }

            Event::Text(text) => {
                out.push_str(&escape_typst(&text));
            }

            Event::SoftBreak => out.push('\n'),
            Event::HardBreak => out.push_str("\\\n"),

            Event::Rule => out.push_str("\n#line(length: 100%, stroke: 0.5pt + gray)\n\n"),

            // Skip images, HTML, footnotes, etc.
            _ => {}
        }
    }

    out.trim().to_string()
}

/// Escape characters that have special meaning in Typst.
fn escape_typst(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '#' | '@' | '$' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

enum ListKind {
    Ordered(u64),
    Unordered,
}

fn heading_depth(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text() {
        assert_eq!(markdown_to_typst("Hello world"), "Hello world");
    }

    #[test]
    fn bold_and_italic() {
        assert_eq!(
            markdown_to_typst("**bold** and _italic_"),
            "*bold* and _italic_"
        );
    }

    #[test]
    fn inline_code() {
        assert_eq!(markdown_to_typst("Use `foo()` here"), "Use `foo()` here");
    }

    #[test]
    fn heading_levels() {
        let input = "# H1\n## H2\n### H3";
        let output = markdown_to_typst(input);
        assert!(output.contains("= H1"));
        assert!(output.contains("== H2"));
        assert!(output.contains("=== H3"));
    }

    #[test]
    fn unordered_list() {
        let input = "- one\n- two\n- three";
        let output = markdown_to_typst(input);
        assert!(output.contains("- one"));
        assert!(output.contains("- two"));
        assert!(output.contains("- three"));
    }

    #[test]
    fn ordered_list() {
        let input = "1. first\n2. second";
        let output = markdown_to_typst(input);
        assert!(output.contains("+ first"));
        assert!(output.contains("+ second"));
    }

    #[test]
    fn link() {
        let input = "[click here](https://example.com)";
        let output = markdown_to_typst(input);
        assert!(output.contains("#link(\"https://example.com\")[click here]"));
    }

    #[test]
    fn code_block() {
        let input = "```\nfn main() {}\n```";
        let output = markdown_to_typst(input);
        assert!(output.contains("```\nfn main() {}\n```"));
    }

    #[test]
    fn escapes_typst_special_chars() {
        assert_eq!(escape_typst("a #b @c $d"), "a \\#b \\@c \\$d");
    }

    #[test]
    fn empty_input() {
        assert_eq!(markdown_to_typst(""), "");
    }

    #[test]
    fn horizontal_rule() {
        let output = markdown_to_typst("---");
        assert!(output.contains("#line("));
    }

    #[test]
    fn strikethrough() {
        assert_eq!(markdown_to_typst("~~removed~~"), "#strike[removed]");
    }

    #[test]
    fn blockquote() {
        let output = markdown_to_typst("> quoted text");
        assert!(output.contains("#block("));
        assert!(output.contains("quoted text"));
    }

    #[test]
    fn paragraphs_separated() {
        let output = markdown_to_typst("Para one.\n\nPara two.");
        assert!(output.contains("Para one.\n\n"));
        assert!(output.contains("Para two."));
    }
}
