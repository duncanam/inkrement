use clap::Parser;
use color_eyre::eyre::Result;

mod cli;
mod pull_changes;

fn main() -> Result<()> {
    cli::Cli::parse().run()
}
