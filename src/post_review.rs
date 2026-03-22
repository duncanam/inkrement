use std::process::Command;

use color_eyre::eyre::{Context, Result, eyre};
use serde::Serialize;

use crate::annotate::{Annotation, DiffSide, InterpretedReview, ReviewDecision};

/// Metadata parsed from an inkrement filename needed to post a review.
pub(crate) struct ReviewTarget {
    /// Repository in "owner/repo" format.
    pub repo: String,
    /// Pull request number.
    pub number: u32,
}

impl ReviewTarget {
    /// Parse from an inkrement filename.
    /// Expected format: "#72 owner-repo [sha] p105 inkrement.pdf"
    pub(crate) fn from_filename(name: &str) -> Result<Self> {
        let rest = name
            .strip_prefix('#')
            .ok_or_else(|| eyre!("filename does not start with #"))?;
        let (number_str, rest) = rest
            .split_once(' ')
            .ok_or_else(|| eyre!("no space after PR number"))?;
        let (repo_dashed, _) = rest
            .split_once(" [")
            .ok_or_else(|| eyre!("no [ delimiter"))?;

        let number: u32 = number_str.parse().wrap_err("invalid PR number")?;

        // The repo has / replaced with - in the filename. We need to restore it.
        // Convention: first dash after the org name is the separator.
        // "Atomic-Industries-crab-rave" -> "Atomic-Industries/crab-rave"
        // This is ambiguous if both org and repo contain dashes. We try to find
        // the org by checking GitHub.
        let repo = restore_repo_name(repo_dashed)?;

        Ok(Self { repo, number })
    }
}

/// Restore "owner/repo" from a dash-separated filename segment.
/// Uses `gh api` to resolve the ambiguity by checking which split is a valid repo.
fn restore_repo_name(dashed: &str) -> Result<String> {
    // Try each possible split position
    let dashes: Vec<usize> = dashed
        .char_indices()
        .filter(|(_, c)| *c == '-')
        .map(|(i, _)| i)
        .collect();

    for &dash_pos in &dashes {
        let owner = &dashed[..dash_pos];
        let repo = &dashed[dash_pos + 1..];
        let candidate = format!("{owner}/{repo}");

        // Check if this is a valid repo
        let output = Command::new("gh")
            .args(["api", &format!("repos/{candidate}"), "-q", ".full_name"])
            .output();

        if let Ok(output) = output {
            let full_name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if output.status.success() && !full_name.is_empty() {
                return Ok(full_name);
            }
        }
    }

    Err(eyre!(
        "could not determine repo from filename segment: {dashed}"
    ))
}

/// The GitHub review event type.
#[derive(Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ReviewEvent {
    Approve,
    RequestChanges,
    Comment,
}

impl From<&ReviewDecision> for ReviewEvent {
    fn from(decision: &ReviewDecision) -> Self {
        match decision {
            ReviewDecision::Approve => Self::Approve,
            ReviewDecision::RequestChanges => Self::RequestChanges,
            ReviewDecision::CommentOnly | ReviewDecision::Unclear => Self::Comment,
        }
    }
}

/// A single inline comment for the GitHub review API.
#[derive(Serialize)]
struct ReviewComment {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
    side: String,
    body: String,
}

impl From<&Annotation> for ReviewComment {
    fn from(annotation: &Annotation) -> Self {
        Self {
            path: annotation.path.clone(),
            line: annotation.line,
            side: match annotation.side {
                DiffSide::Left => "LEFT".to_string(),
                DiffSide::Right => "RIGHT".to_string(),
            },
            body: annotation.body.clone(),
        }
    }
}

/// The full review payload for the GitHub API.
#[derive(Serialize)]
struct ReviewPayload {
    event: ReviewEvent,
    body: String,
    comments: Vec<ReviewComment>,
}

/// Post an interpreted review to GitHub.
pub(crate) fn post_review(target: &ReviewTarget, review: &InterpretedReview) -> Result<()> {
    let event = ReviewEvent::from(&review.decision);

    let comments: Vec<ReviewComment> = review
        .annotations
        .iter()
        .filter(|a| a.line.is_some()) // GitHub requires a line number for inline comments
        .map(ReviewComment::from)
        .collect();

    // Annotations without line numbers become part of the review body
    let body_comments: Vec<String> = review
        .annotations
        .iter()
        .filter(|a| a.line.is_none())
        .map(|a| format!("**{}**: {}", a.path, a.body))
        .collect();

    let body = if body_comments.is_empty() {
        "Review via inkrement".to_string()
    } else {
        format!("Review via inkrement\n\n{}", body_comments.join("\n\n"))
    };

    let payload = ReviewPayload {
        event,
        body,
        comments,
    };

    let payload_json =
        serde_json::to_string(&payload).wrap_err("failed to serialize review payload")?;

    let api_path = format!("repos/{}/pulls/{}/reviews", target.repo, target.number);

    let mut child = Command::new("gh")
        .args(["api", &api_path, "--method", "POST", "--input", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .wrap_err("failed to start gh CLI")?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(payload_json.as_bytes())
            .wrap_err("failed to write review payload to gh CLI")?;
    }

    let output = child
        .wait_with_output()
        .wrap_err("failed to wait for gh CLI")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!("gh api failed: {stderr}"));
    }

    Ok(())
}
