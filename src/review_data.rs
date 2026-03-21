use serde::Serialize;
use unidiff::PatchSet;

use crate::{fetch_files::SourceFiles, pull_changes::PullRequest};

#[derive(Debug, Serialize)]
struct ReviewData {
    title: String,
    repo: String,
    number: u32,
    author: String,
    reviewer: String,
    base_ref: String,
    head_ref: String,
    lines_added: usize,
    lines_removed: usize,
    files: Box<[FileData]>,
}

#[derive(Debug, Serialize)]
struct FileData {
    path: String,
    lang: String,
    hunks: Box<[HunkData]>,
    old_source: Option<String>,
    new_source: Option<String>,
    old_source_label: Option<String>,
    new_source_label: Option<String>,
}

#[derive(Debug, Serialize)]
struct HunkData {
    id: usize,
    lines: Box<[LineData]>,
}

#[derive(Debug, Serialize)]
struct LineData {
    kind: String,
    content: String,
    old_line_no: Option<usize>,
    new_line_no: Option<usize>,
}

/// Infer language name from file extension for Typst raw blocks
fn lang_from_path(path: &str) -> String {
    path.rsplit('.')
        .next()
        .map(|ext| match ext {
            "rs" => "rust",
            "py" => "python",
            "js" => "javascript",
            "ts" => "typescript",
            "tsx" => "tsx",
            "jsx" => "jsx",
            "rb" => "ruby",
            "go" => "go",
            "java" => "java",
            "kt" => "kotlin",
            "c" | "h" => "c",
            "cpp" | "cc" | "cxx" | "hpp" => "cpp",
            "cs" => "csharp",
            "swift" => "swift",
            "sh" | "bash" | "zsh" => "bash",
            "yml" | "yaml" => "yaml",
            "toml" => "toml",
            "json" => "json",
            "md" => "markdown",
            "html" => "html",
            "css" => "css",
            "sql" => "sql",
            "typ" => "typst",
            "jl" => "julia",
            "m" => "matlab",
            other => other,
        })
        .unwrap_or("")
        .to_string()
}

impl ReviewData {
    /// Build review data from a pull request, its parsed diff, source files, and reviewer name
    pub(crate) fn build(
        pr: PullRequest,
        reviewer: &str,
        patch: &PatchSet,
        source_files: &SourceFiles,
    ) -> Self {
        let files = patch
            .files()
            .iter()
            .enumerate()
            .map(|(file_idx, file)| {
                let path = file.path();
                let lang = lang_from_path(&path);

                let old_source = source_files
                    .old
                    .iter()
                    .find(|sf| sf.path == path)
                    .map(|sf| sf.content.clone());

                let new_source = source_files
                    .new
                    .iter()
                    .find(|sf| sf.path == path)
                    .map(|sf| sf.content.clone());

                let old_source_label = old_source
                    .as_ref()
                    .map(|_| format!("source-old-{file_idx}"));

                let new_source_label = new_source
                    .as_ref()
                    .map(|_| format!("source-new-{file_idx}"));

                let hunks = file
                    .hunks()
                    .iter()
                    .enumerate()
                    .map(|(hunk_idx, hunk)| {
                        let lines = hunk
                            .lines()
                            .iter()
                            .map(|line| LineData {
                                kind: if line.is_added() {
                                    "added".to_string()
                                } else if line.is_removed() {
                                    "removed".to_string()
                                } else {
                                    "context".to_string()
                                },
                                content: line.value.clone(),
                                old_line_no: line.source_line_no,
                                new_line_no: line.target_line_no,
                            })
                            .collect();

                        HunkData {
                            id: hunk_idx + 1,
                            lines,
                        }
                    })
                    .collect();

                FileData {
                    path,
                    lang,
                    hunks,
                    old_source,
                    new_source,
                    old_source_label,
                    new_source_label,
                }
            })
            .collect();

        let lines_added = patch.files().iter().map(|f| f.added()).sum();
        let lines_removed = patch.files().iter().map(|f| f.removed()).sum();

        ReviewData {
            title: pr.title,
            repo: pr.repo_name,
            number: pr.number,
            author: pr.author,
            reviewer: reviewer.to_owned(), // we clone because each PDF needs it
            base_ref: pr.base_ref_oid,
            head_ref: pr.head_ref_oid,
            lines_added,
            lines_removed,
            files,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::rust("src/main.rs", "rust")]
    #[case::typescript("app/index.ts", "typescript")]
    #[case::tsx("app/index.tsx", "tsx")]
    #[case::python("deeply/nested/file.py", "python")]
    #[case::javascript("server.js", "javascript")]
    #[case::go("server.go", "go")]
    #[case::java("Main.java", "java")]
    #[case::c_header("lib.h", "c")]
    #[case::cpp("main.cpp", "cpp")]
    #[case::cc("main.cc", "cpp")]
    #[case::hpp("main.hpp", "cpp")]
    #[case::ruby("app.rb", "ruby")]
    #[case::swift("app.swift", "swift")]
    #[case::bash("script.sh", "bash")]
    #[case::yml("config.yml", "yaml")]
    #[case::yaml("config.yaml", "yaml")]
    #[case::toml("Cargo.toml", "toml")]
    #[case::json("data.json", "json")]
    #[case::sql("query.sql", "sql")]
    #[case::typst("template.typ", "typst")]
    #[case::unknown_ext("file.xyz", "xyz")]
    #[case::no_extension("Makefile", "Makefile")]
    fn lang_from_path_cases(#[case] path: &str, #[case] expected: &str) {
        assert_eq!(lang_from_path(path), expected);
    }
}
