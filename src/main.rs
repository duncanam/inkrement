use clap::Parser;
use color_eyre::eyre::{Context, Result};

mod annotate;
mod cli;
mod commands;
mod diff_parse;
mod fetch_files;
mod markdown;
mod pdf;
mod pdf_strip;
mod post_review;
mod pull_changes;
mod remarkable;
mod review_data;
mod tui;

fn main() -> Result<()> {
    color_eyre::install().wrap_err("could not set up error colors")?;
    cli::Cli::parse().run()
}
