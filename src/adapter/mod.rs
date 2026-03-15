pub mod telegram;

use anyhow::Context;
use clap::Args;
use std::path::PathBuf;

// @chunk adapter/options
// Unified adapter entrypoint. Provider selection stays stable while each
// provider can own its specific runtime options.
#[derive(Debug, Clone, Args)]
pub struct AdapterOptions {
    /// Adapter provider name. Supported: telegram.
    #[arg(long, default_value = "telegram")]
    pub provider: String,

    /// Internal startup handshake file used by `kelix start` autostart flow.
    #[arg(long, hide = true)]
    pub ready_file: Option<PathBuf>,

    /// Reset provider state to factory defaults and exit (clears whitelist and
    /// chat bindings). Does not require a bot token or gateway connection.
    #[arg(long)]
    pub reset: bool,

    #[command(flatten)]
    pub telegram: telegram::TelegramOptions,
}
// @end-chunk

pub async fn run(options: AdapterOptions) -> anyhow::Result<()> {
    match options.provider.trim().to_ascii_lowercase().as_str() {
        "telegram" => telegram::run(options.telegram, options.reset, options.ready_file).await,
        other => Err(anyhow::anyhow!(
            "unsupported adapter provider '{}'; currently supported: telegram",
            other
        ))
        .context("invalid adapter provider"),
    }
}
