use clap::Parser;
use color_eyre::eyre::{Context, Result};

mod cli;
mod diff_parse;
mod fetch_files;
mod pdf;
mod pull_changes;
mod remarkable;
mod review_data;

fn main() -> Result<()> {
    color_eyre::install().wrap_err("could not set up error colors")?;
    cli::Cli::parse().run()
}
