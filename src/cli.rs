use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "inkrement",
    about = "Incremental code review, in ink; review PRs on your reMarkable.",
    long_about = "Incremental code review, in ink; review PRs on your reMarkable.\n\n\
        Gets active pull requests from Github and formats them into reMarkable-reviewable PDFs. \
        Then, leverages Anthropic's OCR tools to interpret handwriting and sentence placement into review comments. \
        Lastly, posts a pull request review with the changes written.",
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
    pub fn run(self) {
        match self.command {
            Command::Sync => {
                todo!("implement sync")
            }
        }
    }
}
