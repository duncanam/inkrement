use clap::Parser;
use color_eyre::eyre::Result;

mod cli;

fn main() -> Result<()> {
    cli::Cli::parse().run()
}
