use clap::Parser;
use color_eyre::eyre::Result;

mod cli;
mod diff_parse;
mod fetch_files;
mod pull_changes;

fn main() -> Result<()> {
    cli::Cli::parse().run()
}
