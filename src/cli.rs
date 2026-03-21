use std::{collections::HashSet, fs, path::PathBuf};

use clap::{Parser, Subcommand};
use color_eyre::eyre::{Context, Result};

use crate::{pull_changes::PullRequests, remarkable::RemarkableClient};

#[derive(Parser)]
#[command(
    name = "inkrement",
    about = "Incremental code review, in ink; review PRs on your reMarkable.",
    long_about = "\
Incremental code review, in ink; review PRs on your reMarkable.

Fetches active pull requests from GitHub, formats them into reMarkable-reviewable
PDFs, interprets your handwritten annotations using Anthropic's OCR tools, and
posts a pull request review with your comments.

Workflow:
  1. Run `inkrement sync` to fetch open PRs into PR Reviews/Open
  2. On your reMarkable, move a review to PR Reviews/In Process and annotate
  3. When finished, move it to PR Reviews/Complete
  4. Run `inkrement sync` again to parse and post your review to GitHub

Directory structure (created automatically on your reMarkable):
  PR Reviews/Open         New reviews waiting for markup
  PR Reviews/In Process   Reviews you are actively annotating
  PR Reviews/Complete     Finished reviews ready to post

Required CLI tools: gh, claude",
    arg_required_else_help = true,
    version
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Gets new review changes from Github and posts finished reviews
    Sync,
    /// Generate review PDFs locally for preview (no reMarkable needed)
    Generate {
        /// Output directory for generated PDFs
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
    },
}

impl Cli {
    /// Runs the inkcrement CLI logic
    pub fn run(self) -> Result<()> {
        match self.command {
            Command::Sync => {
                let remarkable = RemarkableClient::connect()
                    .wrap_err("while syncing, could not connect to reMarkable")?;

                let prs = PullRequests::fetch()
                    .wrap_err("while syncing, could not fetch pull requests")?;

                let existing: HashSet<String> = remarkable
                    .list_inkrement_documents()
                    .wrap_err("while syncing, could not get Inkcrement documents")?
                    .into_iter()
                    .map(|d| d.visible_name)
                    .collect();

                for pr in prs.pull_requests {
                    let filename = pr.pdf_filename();

                    if existing.contains(&filename) {
                        println!("Skipping {} (already on tablet)", filename);
                        continue;
                    }

                    println!("Rendering {}...", filename);
                    let pdf = pr.render_to_pdf(&prs.reviewer).wrap_err_with(|| {
                        format!(
                            "while syncing, failed to render {}#{} \"{}\"",
                            pr.repo_name, pr.number, pr.title
                        )
                    })?;

                    println!("Uploading {}...", filename);
                    remarkable.upload(&filename, &pdf).wrap_err_with(|| {
                        format!(
                            "while syncing, failed to upload {}#{} \"{}\"",
                            pr.repo_name, pr.number, pr.title
                        )
                    })?;

                    println!("Uploaded: {}", filename);
                }

                Ok(())
            }

            Command::Generate { output: output_dir } => {
                let prs = PullRequests::fetch()?;

                fs::create_dir_all(&output_dir).wrap_err("failed to create output directory")?;

                for pr in prs.pull_requests.into_vec() {
                    let pdf = pr.render_to_pdf(&prs.reviewer).wrap_err_with(|| {
                        format!(
                            "failed to PDF-render {}#{} \"{}\"",
                            pr.repo_name, pr.number, pr.title
                        )
                    })?;
                    let filename = pr.pdf_filename();
                    pdf.write(&output_dir, &filename).wrap_err_with(|| {
                        format!(
                            "failed to write PDF to file for {}#{} \"{}\"",
                            pr.repo_name, pr.number, pr.title
                        )
                    })?;

                    println!("Generated: {}", output_dir.join(&filename).display());
                }

                Ok(())
            }
        }
    }
}
