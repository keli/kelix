mod adapter;
mod commands;
mod config;
mod error;
mod frontend;
mod gateway;
mod loop_runner;
mod paths;
mod policy;
mod protocol;
mod session;
mod spawn;
mod tui_client;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    commands::run().await
}
