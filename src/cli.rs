use std::path::PathBuf;

use clap::{Parser, Subcommand};
use color_eyre::eyre::Result;

use crate::commands;

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
    /// Gets new pull request reviews from GitHub and uploads them to the reMarkable
    Get,
    /// Generate review PDFs locally for preview (no reMarkable needed)
    Generate {
        /// Output directory for generated PDFs
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
    },
}

impl Cli {
    /// Runs the inkrement CLI logic
    pub fn run(self) -> Result<()> {
        match self.command {
            Command::Get => commands::get_reviews_and_send_to_remarkable(),
            Command::Generate { output } => commands::generate_local_pdfs(&output),
        }
    }
}
