use std::process::Command;

use color_eyre::eyre::{Context, Result, eyre};
use itertools::Itertools;
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
            "-H",
            "Accept: application/vnd.github.raw+json",
        ])
        .output()
        .wrap_err("failed to run gh CLI while attempting to fetch file at ref")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!(
            "failed to fetch file at ref due to gh api failing for {repo_name}/{path}@{git_ref}: {stderr}"
        ));
    }

    String::from_utf8(output.stdout).wrap_err_with(|| {
        format!("failed to fetch file at ref due to non-UTF-8 content in {path}@{git_ref}")
    })
}

impl PullRequest {
    /// Fetches the old and new source files for all files in the given diff
    pub(crate) fn fetch_source_files(&self, patch: &PatchSet) -> Result<SourceFiles> {
        let old = patch
            .files()
            .iter()
            .filter(|file| !file.is_added_file())
            .map(|file| -> Result<SourceFile> {
                let path = file.source_file.strip_prefix("a/")
                    .unwrap_or(&file.source_file)
                    .to_string();

                let content = fetch_file_at_ref(&self.repo_name, &path, &self.base_ref_oid)
                    .wrap_err_with(|| format!("failed to fetch old version of {path}"))?;

                Ok(SourceFile { path, content })
            })
            .try_collect()?;

        let new = patch
            .files()
            .iter()
            .filter(|file| !file.is_removed_file())
            .map(|file| -> Result<SourceFile> {
                let path = file.target_file.strip_prefix("b/")
                    .unwrap_or(&file.target_file)
                    .to_string();

                let content = fetch_file_at_ref(&self.repo_name, &path, &self.head_ref_oid)
                    .wrap_err_with(|| format!("failed to fetch new version of {path}"))?;

                Ok(SourceFile { path, content })
            })
            .try_collect()?;

        Ok(SourceFiles { old, new })
    }
}
