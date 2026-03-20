use std::process::Command;

use color_eyre::eyre::{Context, Result, eyre};
use itertools::Itertools;
use serde::Deserialize;

/// Represents all pull requests found by the PR search JSON output
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

/// Represents a pull request as found by Github search JSON
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

/// Represents a JSON packet from Github with metadata
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestResponse {
    number: u32,
    title: String,
    url: String,
    head_ref_name: String,
    head_ref_oid: String,
    base_ref_name: String,
    base_ref_oid: String,
}

/// A pull request and its metadata
///
/// # Note
/// This differs from PullRequestResponse due to requiring the repo_name which is not in the JSON
/// payload delivered by Github and requires parsing
#[derive(Debug)]
struct PullRequest {
    number: u32,
    title: String,
    repo_name: String,
    url: String,
    head_ref_name: String,
    head_ref_oid: String,
    base_ref_name: String,
    base_ref_oid: String,
}

impl PullRequest {
    /// Fetch the git diff for a specific pull request
    fn fetch_diff(&self) -> Result<String> {
        let output = Command::new("gh")
            .args([
                "pr",
                "diff",
                &self.number.to_string(),
                "--repo",
                &self.repo_name,
            ])
            .output()
            .wrap_err("failed to run gh CLI while attempting to get git diff")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!(
                "while getting git diff, gh pr diff failed for {}#{}: {stderr}",
                self.repo_name,
                self.number
            ));
        }

        String::from_utf8(output.stdout).wrap_err("gh pr diff returned invalid UTF-8")
    }
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

        let resp: PullRequestResponse =
            serde_json::from_slice(&output.stdout).wrap_err_with(|| {
                format!("failed to parse PR {repo_name}#{number} while fetching pull requests")
            })?;

        Ok(PullRequest {
            number: resp.number,
            title: resp.title,
            repo_name: repo_name.to_string(),
            url: resp.url,
            head_ref_name: resp.head_ref_name,
            head_ref_oid: resp.head_ref_oid,
            base_ref_name: resp.base_ref_name,
            base_ref_oid: resp.base_ref_oid,
        })
    }
}

/// All active pull requests
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_search_response() {
        let json = r#"{
            "items": [
                {"number": 42, "repository_url": "https://api.github.com/repos/acme/widgets"},
                {"number": 99, "repository_url": "https://api.github.com/repos/acme/api"}
            ]
        }"#;

        let response: SearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.items.len(), 2);
        assert_eq!(response.items[0].number, 42);
        assert_eq!(response.items[1].number, 99);
    }

    #[test]
    fn parse_search_response_empty() {
        let json = r#"{"items": []}"#;

        let response: SearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.items.len(), 0);
    }

    #[test]
    fn extract_repo_name() {
        let item = SearchPullRequest {
            number: 42,
            repository_url: "https://api.github.com/repos/acme/widgets".to_string(),
        };

        assert_eq!(item.get_repo_name().unwrap(), "acme/widgets");
    }

    #[test]
    fn extract_repo_name_bad_url() {
        let item = SearchPullRequest {
            number: 42,
            repository_url: "https://example.com/unexpected".to_string(),
        };

        assert!(item.get_repo_name().is_err());
    }

    #[test]
    fn parse_pull_request_response() {
        let json = r#"{
            "number": 42,
            "title": "Add widget endpoint",
            "url": "https://github.com/acme/widgets/pull/42",
            "headRefName": "feature/widgets",
            "headRefOid": "abc123def456",
            "baseRefName": "main",
            "baseRefOid": "789fed321cba"
        }"#;

        let resp: PullRequestResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.number, 42);
        assert_eq!(resp.title, "Add widget endpoint");
        assert_eq!(resp.head_ref_name, "feature/widgets");
        assert_eq!(resp.head_ref_oid, "abc123def456");
        assert_eq!(resp.base_ref_name, "main");
        assert_eq!(resp.base_ref_oid, "789fed321cba");
    }
}
