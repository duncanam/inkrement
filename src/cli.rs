use std::path::PathBuf;

use clap::{Parser, Subcommand};
use color_eyre::eyre::Result;

use crate::{commands, tui};

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
  1. Run `inkrement` to launch the interactive TUI
  2. Select PRs to send to your reMarkable
  3. Annotate on the tablet, then plug back in
  4. Switch to the Publish tab to post reviews

Scripting:
  inkrement get        Non-interactive: fetch PRs and upload to reMarkable
  inkrement generate   Generate review PDFs locally for preview
  inkrement download   Download annotated PDFs from reMarkable (stripped)
  inkrement interpret  Run Claude OCR on stripped PDFs, output JSON

Required CLI tools: gh, claude",
    version
)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
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
    /// Download annotated PDFs from reMarkable, strip source pages, and save locally
    Download {
        /// Output directory for downloaded PDFs
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
    },
    /// Interpret annotated PDFs via Claude OCR, saving JSON results alongside each PDF
    Interpret {
        /// Directory containing stripped PDF files to interpret
        #[arg(short, long, default_value = ".")]
        input: PathBuf,
    },
}

impl Cli {
    /// Runs the inkrement CLI logic
    pub fn run(self) -> Result<()> {
        match self.command {
            None => {
                let terminal = ratatui::init();
                let result = tui::App::new().run(terminal);
                ratatui::restore();
                result
            }
            Some(Command::Get) => commands::get_reviews_and_send_to_remarkable(),
            Some(Command::Generate { output }) => commands::generate_local_pdfs(&output),
            Some(Command::Download { output }) => commands::download_annotated_pdfs(&output),
            Some(Command::Interpret { input }) => commands::interpret_pdfs(&input),
        }
    }
}
