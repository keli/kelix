/// SpawnDispatcher: accepts spawn requests from the loop runner,
/// starts worker tasks, and sends results back via mpsc channel.
pub mod process_runner;
pub mod tracker;

use crate::config::Config;
use crate::error::CoreError;
use process_runner::{run_subagent_process, ProcessResult, WorkerStreamChunk};
use tokio::sync::{mpsc, oneshot};
use tracker::SpawnTracker;

pub struct SpawnedResult {
    pub spawn_id: String,
    pub result: ProcessResult,
}

pub struct SpawnDispatcher {
    config: Config,
    tracker: SpawnTracker,
    result_tx: mpsc::Sender<SpawnedResult>,
    stream_tx: mpsc::Sender<(String, WorkerStreamChunk)>,
}

impl SpawnDispatcher {
    pub fn new(
        config: Config,
        result_tx: mpsc::Sender<SpawnedResult>,
        stream_tx: mpsc::Sender<(String, WorkerStreamChunk)>,
    ) -> Self {
        Self {
            config,
            tracker: SpawnTracker::new(),
            result_tx,
            stream_tx,
        }
    }

    /// Attempt to dispatch a spawn request.
    ///
    /// Enforces:
    /// - Subagent must be in config
    /// - max_spawns (if non-zero)
    /// - max_concurrent_spawns (if non-zero)
    ///
    /// Returns `Ok(())` if the spawn was accepted (spawn_ack should be sent).
    /// Returns `Err` if the spawn was rejected (error response should be sent instead).
    pub fn dispatch(
        &mut self,
        spawn_id: String,
        subagent_name: &str,
        input: serde_json::Value,
        total_spawn_count: u64,
    ) -> Result<(), CoreError> {
        let subagent = self
            .config
            .subagents
            .get(subagent_name)
            .ok_or_else(|| CoreError::UnknownSubagent(subagent_name.to_string()))?
            .clone();

        let max_spawns = self.config.agent.max_spawns;
        if max_spawns > 0 && total_spawn_count >= max_spawns {
            return Err(CoreError::SpawnLimitExceeded);
        }

        let max_concurrent = self.config.agent.max_concurrent_spawns;
        if max_concurrent > 0 && self.tracker.len() >= max_concurrent as usize {
            return Err(CoreError::SpawnLimitExceeded);
        }

        let (cancel_tx, cancel_rx) = oneshot::channel();
        let max_output_bytes = self.config.tools.shell.max_output_bytes;
        let result_tx = self.result_tx.clone();
        let stream_tx = self.stream_tx.clone();
        let spawn_id_clone = spawn_id.clone();

        let handle = tokio::spawn(async move {
            let (chunk_tx, mut chunk_rx) = mpsc::channel::<WorkerStreamChunk>(256);
            let forward_spawn_id = spawn_id_clone.clone();
            let forward_stream_tx = stream_tx.clone();
            let forward_task = tokio::spawn(async move {
                while let Some(chunk) = chunk_rx.recv().await {
                    if forward_stream_tx
                        .send((forward_spawn_id.clone(), chunk))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });

            let result = run_subagent_process(
                &subagent,
                &input,
                Some(chunk_tx),
                cancel_rx,
                max_output_bytes,
                10,
            )
            .await;
            let _ = forward_task.await;
            let _ = result_tx
                .send(SpawnedResult {
                    spawn_id: spawn_id_clone,
                    result,
                })
                .await;
            // Return value unused; result delivered via channel
            ProcessResult {
                exit_code: 0,
                raw_stdout: vec![],
                truncated: false,
                process_error: None,
            }
        });

        self.tracker
            .insert(spawn_id, subagent_name.to_string(), cancel_tx, handle);
        Ok(())
    }

    /// Cancel an in-flight spawn. Synchronous: returns after signaling cancellation.
    /// The caller must still await the final result delivery (or simply trust it won't arrive).
    pub fn cancel(&mut self, spawn_id: &str) -> crate::protocol::core_msg::CancelStatus {
        self.tracker.cancel(spawn_id)
    }

    pub fn in_flight_count(&self) -> usize {
        self.tracker.len()
    }

    pub fn is_known_spawn(&self, spawn_id: &str) -> bool {
        self.tracker.contains(spawn_id)
    }

    /// Return the subagent name for an in-flight spawn, if known.
    pub fn subagent_name(&self, spawn_id: &str) -> Option<String> {
        self.tracker.subagent_name(spawn_id)
    }

    /// Mark a spawn as completed (remove from tracker after result delivered).
    pub fn complete(&mut self, spawn_id: &str) {
        self.tracker.remove(spawn_id);
    }
}
