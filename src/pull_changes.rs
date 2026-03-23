use std::process::Command;

use color_eyre::eyre::{Context, Result, eyre};
use itertools::Itertools;
use serde::{Deserialize, Serialize};

use crate::{diff_parse, pdf::Pdf, review_data::ReviewData};

/// Tag embedded in PDF filenames to identify inkrement documents
pub(crate) const INKREMENT_TAG: &str = "inkrement";

/// Represents all pull requests found by the PR search JSON output
#[derive(Debug, Deserialize)]
struct SearchResponse {
    items: Box<[SearchPullRequest]>,
}

impl SearchResponse {
    /// Search for the user's PRs
    fn fetch() -> Result<Self> {
        let output = Command::new("gh")
            .args(["api", "search/issues?q=is:pr+is:open+review-requested:@me"])
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

/// Get the currently logged-in user
fn fetch_current_user() -> Result<String> {
    let output = Command::new("gh")
        .args(["api", "user", "-q", ".login"])
        .output()
        .wrap_err("failed to run gh CLI while getting user")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!("while getting user, gh api user failed: {stderr}"));
    }

    Ok(String::from_utf8(output.stdout)
        .wrap_err("gh api user returned invalid UTF-8")?
        .trim()
        .to_string())
}

/// Represents a JSON packet from Github with metadata
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestResponse {
    number: u32,
    title: String,
    author: GitHubUser,
    additions: usize,
    deletions: usize,
    head_ref_oid: String,
    base_ref_oid: String,
}

#[derive(Debug, Deserialize)]
struct GitHubUser {
    login: String,
}

/// A review from the GitHub pull request reviews API
#[derive(Debug, Deserialize)]
struct Review {
    user: GitHubUser,
    commit_id: String,
    state: String,
}

/// Whether the diff covers the full PR or only changes since the last review
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DiffMode {
    Full,
    Incremental,
}

/// A resolved diff: the diff content, effective base commit, and mode
#[derive(Debug)]
pub(crate) struct ResolvedDiff {
    pub(crate) diff: String,
    pub(crate) diff_base: String,
    pub(crate) mode: DiffMode,
}

/// Find the commit SHA of the reviewer's most recent submitted review.
/// Returns `None` if the reviewer has no submitted (non-pending) reviews.
fn find_last_reviewed_commit<'a>(reviews: &'a [Review], reviewer: &str) -> Option<&'a str> {
    reviews
        .iter()
        .rfind(|r| r.user.login == reviewer && r.state != "PENDING")
        .map(|r| r.commit_id.as_str())
}

/// A pull request and its metadata
///
/// # Note
/// This differs from PullRequestResponse due to requiring the repo_name which is not in the JSON
/// payload delivered by Github and requires parsing
// TODO: Clone is only here to send PR data to the upload worker thread. Fix this.
#[derive(Debug, Clone)]
pub(crate) struct PullRequest {
    pub(crate) number: u32,
    pub(crate) title: String,
    pub(crate) repo_name: String,
    pub(crate) author: String,
    pub(crate) additions: usize,
    pub(crate) deletions: usize,
    pub(crate) head_ref_oid: String,
    pub(crate) base_ref_oid: String,
}

impl PullRequest {
    /// The filename prefix that identifies this PR (without the page count).
    /// Used for duplicate detection - matches regardless of OCR page count.
    pub(crate) fn filename_prefix(&self) -> String {
        let short_sha = &self.head_ref_oid[..8.min(self.head_ref_oid.len())];
        format!(
            "#{} {} [{short_sha}]",
            self.number,
            self.repo_name.replace('/', "-"),
        )
    }

    /// Generate a PDF filename for this PR.
    /// `ocr_pages` is embedded in the filename so it survives the reMarkable round-trip.
    pub(crate) fn pdf_filename(&self, ocr_pages: usize) -> String {
        format!(
            "{} p{ocr_pages} {INKREMENT_TAG}.pdf",
            self.filename_prefix(),
        )
    }

    /// Fetch the full PR diff (base to head)
    fn fetch_full_diff(&self) -> Result<String> {
        let output = Command::new("gh")
            .args([
                "pr",
                "diff",
                &self.number.to_string(),
                "--repo",
                &self.repo_name,
            ])
            .output()
            .wrap_err("failed to run gh CLI while attempting to get full diff")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!(
                "gh pr diff failed for {}#{}: {stderr}",
                self.repo_name,
                self.number
            ));
        }

        String::from_utf8(output.stdout).wrap_err("gh pr diff returned invalid UTF-8")
    }

    /// Fetch an incremental diff between a base commit and the PR head
    fn fetch_compare_diff(&self, base: &str) -> Result<String> {
        let api_path = format!(
            "/repos/{}/compare/{}...{}",
            self.repo_name, base, self.head_ref_oid
        );
        let output = Command::new("gh")
            .args([
                "api",
                &api_path,
                "-H",
                "Accept: application/vnd.github.diff",
            ])
            .output()
            .wrap_err("failed to run gh CLI while fetching compare diff")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!(
                "gh compare diff failed for {}#{}  ({base}...{}): {stderr}",
                self.repo_name,
                self.number,
                self.head_ref_oid
            ));
        }

        String::from_utf8(output.stdout).wrap_err("compare diff returned invalid UTF-8")
    }

    /// Fetch reviews for this PR from the GitHub API
    fn fetch_reviews(&self) -> Result<Vec<Review>> {
        let api_path = format!("/repos/{}/pulls/{}/reviews", self.repo_name, self.number);
        let output = Command::new("gh")
            .args(["api", &api_path])
            .output()
            .wrap_err("failed to run gh CLI while fetching reviews")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!(
                "gh api failed fetching reviews for {}#{}: {stderr}",
                self.repo_name,
                self.number
            ));
        }

        serde_json::from_slice(&output.stdout).wrap_err("failed to parse reviews response")
    }

    /// Resolve which diff to show: incremental since last review, or full PR diff.
    /// Falls back to full diff if there is no prior review or if the compare fails
    /// (e.g. after a force push).
    pub(crate) fn resolve_diff(&self, reviewer: &str) -> Result<ResolvedDiff> {
        let reviews = self
            .fetch_reviews()
            .wrap_err("could not fetch reviews for incremental diff")?;

        if let Some(commit) = find_last_reviewed_commit(&reviews, reviewer) {
            match self.fetch_compare_diff(commit) {
                Ok(diff) => {
                    return Ok(ResolvedDiff {
                        diff,
                        diff_base: commit.to_string(),
                        mode: DiffMode::Incremental,
                    });
                }
                Err(_) => {
                    // Compare failed (likely force push removed the commit), fall through to full diff
                }
            }
        }

        let diff = self
            .fetch_full_diff()
            .wrap_err("could not fetch full diff as fallback")?;
        Ok(ResolvedDiff {
            diff,
            diff_base: self.base_ref_oid.clone(),
            mode: DiffMode::Full,
        })
    }

    /// Render the PR to a PDF
    pub(crate) fn render_to_pdf(&self, reviewer: &str) -> Result<Pdf> {
        let resolved = self.resolve_diff(reviewer)?;
        let patch = diff_parse::parse_diff(&resolved.diff)?;
        let source_files = self.fetch_source_files(&patch, &resolved.diff_base)?;
        let review_data = ReviewData::build(self, reviewer, &patch, &source_files, &resolved);
        Pdf::render(&review_data)
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
                "number,title,author,additions,deletions,headRefOid,baseRefOid",
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
            author: resp.author.login,
            additions: resp.additions,
            deletions: resp.deletions,
            head_ref_oid: resp.head_ref_oid,
            base_ref_oid: resp.base_ref_oid,
        })
    }
}

/// All active pull requests
#[derive(Debug)]
pub(crate) struct PullRequests {
    pub(crate) pull_requests: Box<[PullRequest]>,
    pub(crate) reviewer: String,
}

impl PullRequests {
    /// Get the user's active pull requests
    pub(crate) fn fetch() -> Result<Self> {
        let reviewer = fetch_current_user().wrap_err("could not get current user for reviewer")?;

        let search =
            SearchResponse::fetch().wrap_err("could not get list of active pull requests")?;

        let pull_requests = search
            .items
            .into_iter()
            .map(PullRequest::try_from)
            .try_collect()
            .wrap_err("could not parse active pull requests")?;

        Ok(Self {
            pull_requests,
            reviewer,
        })
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

    // ========================================================================
    // Review resolution
    // ========================================================================

    #[test]
    fn find_last_reviewed_single_review() {
        let reviews = [Review {
            user: GitHubUser {
                login: "alice".to_string(),
            },
            commit_id: "abc123".to_string(),
            state: "APPROVED".to_string(),
        }];
        assert_eq!(find_last_reviewed_commit(&reviews, "alice"), Some("abc123"));
    }

    #[test]
    fn find_last_reviewed_returns_most_recent() {
        let reviews = [
            Review {
                user: GitHubUser {
                    login: "alice".to_string(),
                },
                commit_id: "first".to_string(),
                state: "COMMENTED".to_string(),
            },
            Review {
                user: GitHubUser {
                    login: "alice".to_string(),
                },
                commit_id: "second".to_string(),
                state: "APPROVED".to_string(),
            },
        ];
        assert_eq!(find_last_reviewed_commit(&reviews, "alice"), Some("second"));
    }

    #[test]
    fn find_last_reviewed_empty_reviews() {
        let reviews: [Review; 0] = [];
        assert_eq!(find_last_reviewed_commit(&reviews, "alice"), None);
    }

    #[test]
    fn find_last_reviewed_no_matching_reviewer() {
        let reviews = [Review {
            user: GitHubUser {
                login: "bob".to_string(),
            },
            commit_id: "abc123".to_string(),
            state: "APPROVED".to_string(),
        }];
        assert_eq!(find_last_reviewed_commit(&reviews, "alice"), None);
    }

    #[test]
    fn find_last_reviewed_skips_pending() {
        let reviews = [
            Review {
                user: GitHubUser {
                    login: "alice".to_string(),
                },
                commit_id: "submitted".to_string(),
                state: "APPROVED".to_string(),
            },
            Review {
                user: GitHubUser {
                    login: "alice".to_string(),
                },
                commit_id: "pending".to_string(),
                state: "PENDING".to_string(),
            },
        ];
        assert_eq!(
            find_last_reviewed_commit(&reviews, "alice"),
            Some("submitted")
        );
    }

    #[test]
    fn find_last_reviewed_filters_by_reviewer() {
        let reviews = [
            Review {
                user: GitHubUser {
                    login: "bob".to_string(),
                },
                commit_id: "bob_commit".to_string(),
                state: "APPROVED".to_string(),
            },
            Review {
                user: GitHubUser {
                    login: "alice".to_string(),
                },
                commit_id: "alice_first".to_string(),
                state: "CHANGES_REQUESTED".to_string(),
            },
            Review {
                user: GitHubUser {
                    login: "bob".to_string(),
                },
                commit_id: "bob_second".to_string(),
                state: "APPROVED".to_string(),
            },
            Review {
                user: GitHubUser {
                    login: "alice".to_string(),
                },
                commit_id: "alice_second".to_string(),
                state: "APPROVED".to_string(),
            },
        ];
        assert_eq!(
            find_last_reviewed_commit(&reviews, "alice"),
            Some("alice_second")
        );
    }

    #[test]
    fn find_last_reviewed_all_non_pending_states_count() {
        let reviews = [
            Review {
                user: GitHubUser {
                    login: "alice".to_string(),
                },
                commit_id: "c1".to_string(),
                state: "COMMENTED".to_string(),
            },
            Review {
                user: GitHubUser {
                    login: "alice".to_string(),
                },
                commit_id: "c2".to_string(),
                state: "CHANGES_REQUESTED".to_string(),
            },
            Review {
                user: GitHubUser {
                    login: "alice".to_string(),
                },
                commit_id: "c3".to_string(),
                state: "APPROVED".to_string(),
            },
            Review {
                user: GitHubUser {
                    login: "alice".to_string(),
                },
                commit_id: "c4".to_string(),
                state: "DISMISSED".to_string(),
            },
        ];
        assert_eq!(find_last_reviewed_commit(&reviews, "alice"), Some("c4"));
    }

    #[test]
    fn parse_reviews_api_response() {
        let json = r#"[
            {
                "id": 1,
                "user": {"login": "alice"},
                "commit_id": "abc123def456",
                "state": "APPROVED",
                "submitted_at": "2024-01-15T10:30:00Z",
                "body": "Looks good!"
            },
            {
                "id": 2,
                "user": {"login": "bob"},
                "commit_id": "789fed321cba",
                "state": "CHANGES_REQUESTED",
                "submitted_at": "2024-01-15T11:00:00Z",
                "body": "Please fix the error handling"
            }
        ]"#;

        let reviews: Vec<Review> = serde_json::from_str(json).unwrap();
        assert_eq!(reviews.len(), 2);
        assert_eq!(reviews[0].user.login, "alice");
        assert_eq!(reviews[0].commit_id, "abc123def456");
        assert_eq!(reviews[0].state, "APPROVED");
        assert_eq!(reviews[1].user.login, "bob");
        assert_eq!(reviews[1].state, "CHANGES_REQUESTED");
    }

    #[test]
    fn diff_mode_serializes_to_snake_case() {
        assert_eq!(serde_json::to_string(&DiffMode::Full).unwrap(), "\"full\"");
        assert_eq!(
            serde_json::to_string(&DiffMode::Incremental).unwrap(),
            "\"incremental\""
        );
    }

    #[test]
    fn parse_pull_request_response() {
        let json = r#"{
            "number": 42,
            "title": "Add widget endpoint",
            "author": {"login": "jsmith"},
            "additions": 150,
            "deletions": 30,
            "headRefOid": "abc123def456",
            "baseRefOid": "789fed321cba"
        }"#;

        let resp: PullRequestResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.number, 42);
        assert_eq!(resp.title, "Add widget endpoint");
        assert_eq!(resp.author.login, "jsmith");
        assert_eq!(resp.additions, 150);
        assert_eq!(resp.deletions, 30);
        assert_eq!(resp.head_ref_oid, "abc123def456");
        assert_eq!(resp.base_ref_oid, "789fed321cba");
    }
}
