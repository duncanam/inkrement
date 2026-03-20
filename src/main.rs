use clap::Parser;

mod cli;

fn main() {
    cli::Cli::parse().run();
}
