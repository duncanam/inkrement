use clap::{Parser, Subcommand};
use color_eyre::eyre::Result;

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
}

impl Cli {
    /// Runs the inkcrement CLI logic
    pub fn run(self) -> Result<()> {
        match self.command {
            Command::Sync => {
                todo!("implement sync")
            }
        }
    }
}
