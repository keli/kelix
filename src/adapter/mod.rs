pub mod telegram;

use anyhow::Context;
use clap::Args;

// @chunk adapter/options
// Unified adapter entrypoint. Provider selection stays stable while each
// provider can own its specific runtime options.
#[derive(Debug, Clone, Args)]
pub struct AdapterOptions {
    /// Adapter provider name. Supported: telegram.
    #[arg(long, default_value = "telegram")]
    pub provider: String,

    #[command(flatten)]
    pub telegram: telegram::TelegramOptions,
}
// @end-chunk

pub async fn run(options: AdapterOptions) -> anyhow::Result<()> {
    match options.provider.trim().to_ascii_lowercase().as_str() {
        "telegram" => telegram::run(options.telegram).await,
        other => Err(anyhow::anyhow!(
            "unsupported adapter provider '{}'; currently supported: telegram",
            other
        ))
        .context("invalid adapter provider"),
    }
}
