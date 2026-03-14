use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

use crate::protocol::adapter_msg::{AdapterMessage, AdapterOutboundMessage};

use super::{route_message, HeadlessState};

// @chunk headless/event-writer-task
// Reads outbound adapter messages from the channel and writes each as a JSON line
// to stdout. Exits when the channel is closed (all senders dropped).
pub async fn event_writer_task(mut rx: mpsc::Receiver<AdapterOutboundMessage>) {
    use tokio::io::AsyncWriteExt;
    let mut stdout = tokio::io::stdout();
    while let Some(event) = rx.recv().await {
        if let Ok(mut line) = serde_json::to_vec(&event) {
            line.push(b'\n');
            let _ = stdout.write_all(&line).await;
            let _ = stdout.flush().await;
        }
    }
}
// @end-chunk

// @chunk headless/stdin-reader-task
// Reads AdapterMessage lines from stdin and routes them to waiting callers.
pub async fn stdin_reader_task(
    state: Arc<Mutex<HeadlessState>>,
    event_tx: mpsc::Sender<AdapterOutboundMessage>,
) {
    use tokio::io::AsyncBufReadExt;
    let stdin = tokio::io::stdin();
    let mut reader = tokio::io::BufReader::new(stdin).lines();

    loop {
        match reader.next_line().await {
            Ok(Some(line)) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<AdapterMessage>(trimmed) {
                    Ok(msg) => route_message(msg, &state, &event_tx).await,
                    Err(e) => {
                        eprintln!("headless: failed to parse adapter message: {e}: {trimmed}");
                    }
                }
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("headless: stdin read error: {e}");
                break;
            }
        }
    }
}
// @end-chunk
