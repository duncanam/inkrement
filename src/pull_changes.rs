use std::process::Command;

use color_eyre::eyre::{Context, Result, eyre};
use itertools::Itertools;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct SearchResponse {
    items: Box<[SearchPullRequest]>,
}

impl SearchResponse {
    /// Search for the user's PRs
    fn fetch() -> Result<Self> {
        let output = Command::new("gh")
            .args([
                "api",
                "/search/issues",
                "-f",
                "q=is:pr is:open review-requested:@me",
            ])
            .output()
            .wrap_err("failed to run gh CLI")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!("gh search failed: {stderr}"));
        }

        serde_json::from_slice(&output.stdout).wrap_err("failed to parse search response")
    }
}

#[derive(Debug, Deserialize)]
struct SearchPullRequest {
    number: u32,
    repository_url: String,
}

impl SearchPullRequest {
    /// Extracts the repo name
    fn get_repo_name(&self) -> Result<&str> {
        self.repository_url
            .strip_prefix("https://api.github.com/repos/")
            .ok_or_else(|| eyre!("unexpected repository_url format: {}", self.repository_url))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequest {
    number: u32,
    title: String,
    url: String,
    head_ref_name: String,
    head_ref_oid: String,
    base_ref_name: String,
    base_ref_oid: String,
}

impl TryFrom<SearchPullRequest> for PullRequest {
    type Error = color_eyre::eyre::Error;
    fn try_from(pr: SearchPullRequest) -> std::result::Result<Self, Self::Error> {
        let number = pr.number;
        let repo_name = pr.get_repo_name()?;

        let output = Command::new("gh")
            .args([
                "pr",
                "view",
                &number.to_string(),
                "--repo",
                repo_name,
                "--json",
                "number,title,url,headRefName,headRefOid,baseRefName,baseRefOid",
            ])
            .output()
            .wrap_err("failed to run gh CLI while fetching pull requests")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!(
                "while fetching pull requests, gh pr view failed for {repo_name}#{number}: {stderr}"
            ));
        }

        serde_json::from_slice(&output.stdout).wrap_err_with(|| {
            format!("failed to parse PR {repo_name}#{number} while fetching pull requests")
        })
    }
}

#[derive(Debug)]
struct PullRequests(Box<[PullRequest]>);

impl PullRequests {
    /// Get the user's active pull requests
    fn fetch() -> Result<Self> {
        let search =
            SearchResponse::fetch().wrap_err("could not get list of active pull requests")?;

        let prs = search
            .items
            .into_iter()
            .map(PullRequest::try_from)
            .try_collect()
            .wrap_err("could not parse active pull requests")?;

        Ok(Self(prs))
    }
}
