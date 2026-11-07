//! Binary entry point: parse the command line and hand off to the library.

use clap::Parser;
use gsd_app::cli::{self, Cli};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Cli::parse();
    cli::init_tracing();
    cli::run(args).await?;
    Ok(())
}
