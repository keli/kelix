/// SpawnTracker: maintains the map of in-flight spawns and handles cancellation.
use crate::protocol::core_msg::CancelStatus;
use std::collections::HashMap;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::process_runner::ProcessResult;

pub struct InFlightSpawn {
    pub spawn_id: String,
    pub subagent: String,
    cancel_tx: Option<oneshot::Sender<()>>,
    pub handle: JoinHandle<ProcessResult>,
}

pub struct SpawnTracker {
    in_flight: HashMap<String, InFlightSpawn>,
}

impl SpawnTracker {
    pub fn new() -> Self {
        Self {
            in_flight: HashMap::new(),
        }
    }

    pub fn insert(
        &mut self,
        spawn_id: String,
        subagent: String,
        cancel_tx: oneshot::Sender<()>,
        handle: JoinHandle<ProcessResult>,
    ) {
        self.in_flight.insert(
            spawn_id.clone(),
            InFlightSpawn {
                spawn_id,
                subagent,
                cancel_tx: Some(cancel_tx),
                handle,
            },
        );
    }

    pub fn len(&self) -> usize {
        self.in_flight.len()
    }

    pub fn is_empty(&self) -> bool {
        self.in_flight.is_empty()
    }

    pub fn contains(&self, spawn_id: &str) -> bool {
        self.in_flight.contains_key(spawn_id)
    }

    /// Remove the spawn entry (called when spawn_result is delivered).
    pub fn remove(&mut self, spawn_id: &str) {
        self.in_flight.remove(spawn_id);
    }

    /// Send SIGTERM to the worker. Returns the cancellation status.
    ///
    /// - `Cancelled`: cancel signal sent; the worker task will deliver a ProcessResult
    ///   with `process_error: Some("cancelled")`. No `spawn_result` will be delivered
    ///   to the orchestrator for this spawn.
    /// - `AlreadyDone`: the spawn_id is not in the tracker (already completed or never known).
    pub fn cancel(&mut self, spawn_id: &str) -> CancelStatus {
        match self.in_flight.get_mut(spawn_id) {
            Some(entry) => {
                if let Some(tx) = entry.cancel_tx.take() {
                    let _ = tx.send(());
                }
                self.in_flight.remove(spawn_id);
                CancelStatus::Cancelled
            }
            None => CancelStatus::AlreadyDone,
        }
    }

    /// Return the subagent name for a given spawn_id, if it is in-flight.
    pub fn subagent_name(&self, spawn_id: &str) -> Option<String> {
        self.in_flight.get(spawn_id).map(|e| e.subagent.clone())
    }

    /// Iterate over all in-flight spawn IDs.
    pub fn spawn_ids(&self) -> Vec<String> {
        self.in_flight.keys().cloned().collect()
    }
}

impl Default for SpawnTracker {
    fn default() -> Self {
        Self::new()
    }
}
