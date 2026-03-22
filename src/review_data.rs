use serde::Serialize;
use unidiff::{Hunk, Line, PatchSet, PatchedFile};

use crate::{fetch_files::SourceFiles, pull_changes::PullRequest};

/// Infer language name from file extension for Typst raw blocks
fn lang_from_path(path: &str) -> &'static str {
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
            _ => "",
        })
        .unwrap_or("")
}

#[derive(Debug, Serialize)]
pub(crate) struct ReviewData<'a> {
    title: &'a str,
    repo: &'a str,
    number: u32,
    author: &'a str,
    reviewer: &'a str,
    base_ref: &'a str,
    head_ref: &'a str,
    lines_added: usize,
    lines_removed: usize,
    files: Box<[FileData<'a>]>,
}

impl<'a> ReviewData<'a> {
    /// Build review data from a pull request, its parsed diff, source files, and reviewer name
    pub(crate) fn build(
        pr: &'a PullRequest,
        reviewer: &'a str,
        patch: &'a PatchSet,
        source_files: &'a SourceFiles,
    ) -> Self {
        let files: Box<_> = patch
            .files()
            .iter()
            .enumerate()
            .map(|(idx, file)| FileData::new(idx, file, source_files))
            .collect();

        let lines_added = files.iter().map(|f| f.added).sum();
        let lines_removed = files.iter().map(|f| f.removed).sum();

        Self {
            title: &pr.title,
            repo: &pr.repo_name,
            number: pr.number,
            author: &pr.author,
            reviewer,
            base_ref: &pr.base_ref_oid,
            head_ref: &pr.head_ref_oid,
            lines_added,
            lines_removed,
            files,
        }
    }
}

#[derive(Debug, Serialize)]
struct FileData<'a> {
    path: String,
    lang: &'a str,
    added: usize,
    removed: usize,
    hunks: Box<[HunkData<'a>]>,
    old_source: Option<&'a str>,
    new_source: Option<&'a str>,
    old_source_label: Option<String>,
    new_source_label: Option<String>,
}

impl<'a> FileData<'a> {
    fn new(idx: usize, file: &'a PatchedFile, source_files: &'a SourceFiles) -> Self {
        let path = file.path();
        let lang = lang_from_path(&path);

        let old_source = source_files
            .old
            .iter()
            .find(|sf| sf.path == path)
            .map(|sf| sf.content.as_str());

        let new_source = source_files
            .new
            .iter()
            .find(|sf| sf.path == path)
            .map(|sf| sf.content.as_str());

        let old_source_label = old_source.map(|_| format!("source-old-{idx}"));
        let new_source_label = new_source.map(|_| format!("source-new-{idx}"));

        let hunks = file
            .hunks()
            .iter()
            .enumerate()
            .map(|(idx_hunk, hunk)| HunkData::new(idx_hunk + 1, hunk))
            .collect();

        Self {
            path,
            lang,
            added: file.added(),
            removed: file.removed(),
            hunks,
            old_source,
            new_source,
            old_source_label,
            new_source_label,
        }
    }
}

#[derive(Debug, Serialize)]
struct HunkData<'a> {
    id: usize,
    lines: Box<[LineData<'a>]>,
}

impl<'a> HunkData<'a> {
    fn new(id: usize, hunk: &'a Hunk) -> Self {
        let lines = hunk.lines().iter().map(LineData::from).collect();
        Self { id, lines }
    }
}

#[derive(Debug, Serialize)]
struct LineData<'a> {
    kind: &'a str,
    content: &'a str,
    old_line_no: Option<usize>,
    new_line_no: Option<usize>,
}

impl<'a> From<&'a Line> for LineData<'a> {
    fn from(line: &'a Line) -> Self {
        let kind = if line.is_added() {
            "added"
        } else if line.is_removed() {
            "removed"
        } else {
            "context"
        };

        Self {
            kind,
            content: &line.value,
            old_line_no: line.source_line_no,
            new_line_no: line.target_line_no,
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
    #[case::unknown_ext("file.xyz", "")]
    #[case::no_extension("Makefile", "")]
    fn lang_from_path_cases(#[case] path: &str, #[case] expected: &str) {
        assert_eq!(lang_from_path(path), expected);
    }
}
