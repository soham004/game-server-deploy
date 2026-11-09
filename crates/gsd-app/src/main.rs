//! Binary entry point: parse the command line and hand off to the library.

use clap::Parser;
use gsd_app::cli::{self, Cli};

// A plain `main` rather than `#[tokio::main]`: the sandbox launcher must run before any runtime
// starts its threads.
fn main() -> anyhow::Result<()> {
    gsd_core::supervisor::sandbox::launch_if_asked();
    let args = Cli::parse();
    cli::init_tracing();
    tokio::runtime::Runtime::new()?.block_on(cli::run(args))?;
    Ok(())
}
