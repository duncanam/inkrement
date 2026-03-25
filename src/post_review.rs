use std::{collections::HashSet, process::Command};

use color_eyre::eyre::{Context, Result, eyre};
use serde::Serialize;
use unidiff::PatchSet;

use crate::annotate::{Annotation, DiffSide, InterpretedReview, ReviewDecision};

const REVIEW_ATTRIBUTION: &str = "Review via [Inkrement](https://github.com/duncanam/inkrement)";

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

/// The full review payload for the GitHub API.
#[derive(Serialize)]
struct ReviewPayload {
    event: ReviewEvent,
    body: String,
    comments: Vec<ReviewComment>,
}

/// Fetch the diff for a PR and return the set of valid (path, line, side) positions.
/// GitHub only accepts inline comments on lines that appear in the diff.
fn fetch_valid_positions(target: &ReviewTarget) -> Result<HashSet<(String, usize, String)>> {
    let output = Command::new("gh")
        .args([
            "pr",
            "diff",
            &target.number.to_string(),
            "--repo",
            &target.repo,
        ])
        .output()
        .wrap_err("failed to fetch diff for review validation")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!("gh pr diff failed: {stderr}"));
    }

    let diff = String::from_utf8(output.stdout).wrap_err("diff is not valid UTF-8")?;
    let mut patch = PatchSet::new();
    patch.parse(&diff).wrap_err("failed to parse diff")?;

    let mut positions = HashSet::new();
    for file in patch.files() {
        let path = file.path();
        for hunk in file.hunks() {
            for line in hunk.lines() {
                if let Some(no) = line.source_line_no {
                    positions.insert((path.clone(), no, "LEFT".to_string()));
                }
                if let Some(no) = line.target_line_no {
                    positions.insert((path.clone(), no, "RIGHT".to_string()));
                }
            }
        }
    }

    Ok(positions)
}

/// Resolve the correct side for an annotation against the diff.
/// Returns the side string if the line is in the diff, preferring the annotated side
/// but falling back to the other side if needed. Returns `None` if the line doesn't
/// exist on either side.
fn resolve_side(
    annotation: &Annotation,
    valid_positions: &HashSet<(String, usize, String)>,
) -> Option<String> {
    let (path, line) = (annotation.path.as_ref()?, annotation.line?);
    let (preferred, other) = match annotation.side {
        DiffSide::Left => ("LEFT", "RIGHT"),
        DiffSide::Right => ("RIGHT", "LEFT"),
    };
    if valid_positions.contains(&(path.clone(), line, preferred.to_string())) {
        Some(preferred.to_string())
    } else if valid_positions.contains(&(path.clone(), line, other.to_string())) {
        Some(other.to_string())
    } else {
        None
    }
}

/// Post an interpreted review to GitHub.
pub(crate) fn post_review(target: &ReviewTarget, review: &InterpretedReview) -> Result<()> {
    let event = ReviewEvent::from(&review.decision);

    let valid_positions = fetch_valid_positions(target)
        .wrap_err("could not validate comment positions against diff")?;

    // Resolve each annotation against the diff, correcting the side if needed
    let mut comments = Vec::new();
    let mut unresolvable = Vec::new();

    for annotation in &review.annotations {
        if let Some(resolved_side) = resolve_side(annotation, &valid_positions) {
            comments.push(ReviewComment {
                path: annotation
                    .path
                    .clone()
                    .expect("resolved annotation has path"),
                line: annotation.line,
                side: resolved_side,
                body: annotation.body.clone(),
            });
        } else {
            unresolvable.push(annotation);
        }
    }

    // Unresolvable annotations become part of the review body
    let body_comments: Vec<String> = unresolvable
        .iter()
        .map(|a| match (&a.path, a.line) {
            (Some(path), Some(line)) => format!(
                "**{path}** (line {line}, could not resolve in diff): {}",
                a.body
            ),
            (Some(path), None) => format!("**{path}**: {}", a.body),
            _ => a.body.clone(),
        })
        .collect();

    let body = if body_comments.is_empty() {
        REVIEW_ATTRIBUTION.to_string()
    } else {
        format!("{REVIEW_ATTRIBUTION}\n\n{}", body_comments.join("\n\n"))
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
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(eyre!("gh api failed: {stderr}\nResponse: {stdout}"));
    }

    Ok(())
}
