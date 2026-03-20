use std::process::Command;

use color_eyre::eyre::{Context, Result, eyre};
use unidiff::PatchSet;

use crate::pull_changes::PullRequest;

/// A source file's contents at a specific commit
#[derive(Debug)]
pub(crate) struct SourceFile {
    pub path: String,
    pub content: String,
}

/// Old and new versions of all files in a diff
#[derive(Debug)]
pub(crate) struct SourceFiles {
    pub old: Box<[SourceFile]>,
    pub new: Box<[SourceFile]>,
}

/// Fetches a single file's contents at a given ref via `gh api`
fn fetch_file_at_ref(repo_name: &str, path: &str, git_ref: &str) -> Result<String> {
    let api_path = format!("/repos/{repo_name}/contents/{path}?ref={git_ref}");
    let output = Command::new("gh")
        .args([
            "api",
            &api_path,
            "-q",
            ".content",
            "-H",
            "Accept: application/vnd.github.raw+json",
        ])
        .output()
        .wrap_err("failed to run gh CLI")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!(
            "gh api failed for {repo_name}/{path}@{git_ref}: {stderr}"
        ));
    }

    String::from_utf8(output.stdout)
        .wrap_err_with(|| format!("non-UTF-8 content in {path}@{git_ref}"))
}

impl PullRequest {
    /// Fetches the old and new source files for all files in the given diff
    pub(crate) fn fetch_source_files(&self, patch: &PatchSet) -> Result<SourceFiles> {
        let mut old = Vec::new();
        let mut new = Vec::new();

        for file in patch.files() {
            let path = file.path();

            if !file.is_added_file() {
                let content = fetch_file_at_ref(&self.repo_name, &path, &self.base_ref_oid)
                    .wrap_err_with(|| format!("failed to fetch old version of {path}"))?;

                old.push(SourceFile {
                    path: path.clone(),
                    content,
                });
            }

            if !file.is_removed_file() {
                let content = fetch_file_at_ref(&self.repo_name, &path, &self.head_ref_oid)
                    .wrap_err_with(|| format!("failed to fetch new version of {path}"))?;

                new.push(SourceFile {
                    path: path.clone(),
                    content,
                });
            }
        }

        Ok(SourceFiles {
            old: old.into(),
            new: new.into(),
        })
    }
}
