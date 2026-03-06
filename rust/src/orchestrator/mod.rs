//! Orchestrator state machine (SPEC §7)
//!
//! Single-authority event loop that owns all runtime state.

mod state;
mod dispatch;
mod retry;

pub use state::{OrchestratorState, RunningEntry};
pub use dispatch::select_candidates;
pub use retry::{compute_backoff, ExitType};

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::AppConfig;
use crate::domain::{Issue, RetryEntry};
use crate::tracker::Tracker;
use crate::agent::{AgentRunner, AgentUpdate};
use crate::observability::RuntimeSnapshot;
use crate::workspace::{run_before_run_hook, run_after_run_hook};

/// Messages sent to the orchestrator
#[derive(Debug)]
pub enum OrchestratorMsg {
    /// Poll tick
    Tick,
    /// Worker finished
    WorkerFinished {
        issue_id: String,
        result: Result<(), crate::agent::AgentError>,
    },
    /// Agent update
    AgentUpdate {
        issue_id: String,
        update: AgentUpdate,
    },
    /// Retry timer fired
    RetryIssue {
        issue_id: String,
    },
    /// Config reloaded
    ConfigReloaded,
    /// Request for runtime snapshot
    SnapshotRequest {
        reply: tokio::sync::oneshot::Sender<RuntimeSnapshot>,
    },
    /// Request refresh
    RefreshRequest {
        reply: tokio::sync::oneshot::Sender<()>,
    },
    /// Shutdown requested
    Shutdown,
}

/// Orchestrator that manages issue dispatch and agent execution
pub struct Orchestrator<T: Tracker, A: AgentRunner> {
    tracker: T,
    agent_runner: Arc<A>,
    config: AppConfig,
    tx: mpsc::UnboundedSender<OrchestratorMsg>,
    rx: UnboundedReceiver<OrchestratorMsg>,
}

impl<T: Tracker + 'static, A: AgentRunner + 'static> Orchestrator<T, A> {
    /// Create a new orchestrator
    pub fn new(tracker: T, agent_runner: A, config: AppConfig) -> (Self, mpsc::UnboundedSender<OrchestratorMsg>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let orchestrator = Self {
            tracker,
            agent_runner: Arc::new(agent_runner),
            config,
            tx: tx.clone(),
            rx,
        };
        (orchestrator, tx)
    }

    /// Run the orchestrator event loop
    pub async fn run(mut self, cancel: CancellationToken) {
        let mut state = OrchestratorState::new(&self.config);
        let mut interval = tokio::time::interval(Duration::from_millis(self.config.polling.interval_ms));

        info!("Orchestrator started");

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("Shutdown requested");
                    // Cancel all running tasks
                    for (_, entry) in state.running.iter() {
                        entry.cancel_token.cancel();
                    }
                    break;
                }

                _ = interval.tick() => {
                    self.handle_tick(&mut state).await;
                }

                Some(msg) = self.rx.recv() => {
                    match msg {
                        OrchestratorMsg::Shutdown => {
                            info!("Shutdown message received");
                            cancel.cancel();
                        }
                        OrchestratorMsg::Tick => {
                            self.handle_tick(&mut state).await;
                        }
                        OrchestratorMsg::WorkerFinished { issue_id, result } => {
                            self.handle_worker_finished(&mut state, issue_id, result).await;
                        }
                        OrchestratorMsg::AgentUpdate { issue_id, update } => {
                            self.handle_agent_update(&mut state, issue_id, update);
                        }
                        OrchestratorMsg::RetryIssue { issue_id } => {
                            self.handle_retry(&mut state, issue_id).await;
                        }
                        OrchestratorMsg::ConfigReloaded => {
                            info!("Config reloaded");
                        }
                        OrchestratorMsg::SnapshotRequest { reply } => {
                            let snapshot = state.to_snapshot();
                            let _ = reply.send(snapshot);
                        }
                        OrchestratorMsg::RefreshRequest { reply } => {
                            self.handle_tick(&mut state).await;
                            let _ = reply.send(());
                        }
                    }
                }
            }
        }

        info!("Orchestrator stopped");
    }

    async fn handle_tick(&self, state: &mut OrchestratorState) {
        // Fetch candidate issues
        match self.tracker.fetch_candidate_issues().await {
            Ok(candidates) => {
                // Select candidates to dispatch
                let to_dispatch = select_candidates(
                    &candidates,
                    &state.running,
                    &state.claimed,
                    &state.retry_attempts,
                    state.max_concurrent_agents,
                );

                // Dispatch selected issues
                for issue in to_dispatch {
                    self.dispatch_issue(state, issue).await;
                }

                // Reconcile running issues
                self.reconcile(state, &candidates).await;
            }
            Err(e) => {
                warn!("Failed to fetch candidates: {}", e);
            }
        }
    }

    async fn dispatch_issue(&self, state: &mut OrchestratorState, issue: Issue) {
        let issue_id = issue.id.clone();
        let identifier = issue.identifier.clone();

        info!(issue_id = %issue_id, identifier = %identifier, "Dispatching issue");

        // Mark as claimed
        state.claimed.insert(issue_id.clone());

        let cancel_token = CancellationToken::new();
        let cancel_clone = cancel_token.clone();

        let tx = self.tx.clone();
        let config = self.config.clone();
        let agent_runner = Arc::clone(&self.agent_runner);
        let issue_clone = issue.clone();
        let attempt = state.retry_attempts.get(&issue_id).map(|r| r.attempt);

        let (update_tx, mut update_rx) = mpsc::unbounded_channel::<(String, AgentUpdate)>();

        // Forward agent updates to the orchestrator channel
        let tx_forward = tx.clone();
        tokio::spawn(async move {
            while let Some((id, update)) = update_rx.recv().await {
                let _ = tx_forward.send(OrchestratorMsg::AgentUpdate { issue_id: id, update });
            }
        });

        let tx_finish = tx.clone();
        let workspace_path = config.workspace.root.join(issue_clone.sanitized_identifier());
        let hooks = config.hooks.clone();

        let task_handle = tokio::spawn(async move {
            // Run before_run hook (fatal on failure)
            if let Err(e) = run_before_run_hook(&workspace_path, &hooks).await {
                warn!("before_run hook failed: {}", e);
                let _ = tx_finish.send(OrchestratorMsg::WorkerFinished {
                    issue_id: issue_clone.id.clone(),
                    result: Err(crate::agent::AgentError::SpawnFailed(e.to_string())),
                });
                return;
            }

            let result = agent_runner
                .run(&issue_clone, attempt, &config, update_tx, cancel_clone)
                .await;

            // Run after_run hook (non-fatal)
            run_after_run_hook(&workspace_path, &hooks).await;

            let _ = tx_finish.send(OrchestratorMsg::WorkerFinished {
                issue_id: issue_clone.id.clone(),
                result,
            });
        });

        let entry = RunningEntry {
            task_handle: Some(task_handle),
            identifier,
            issue,
            cancel_token,
            started_at: chrono::Utc::now(),
            ..Default::default()
        };

        state.running.insert(issue_id, entry);
    }

    async fn handle_worker_finished(
        &self,
        state: &mut OrchestratorState,
        issue_id: String,
        result: Result<(), crate::agent::AgentError>,
    ) {
        if let Some(entry) = state.running.remove(&issue_id) {
            let identifier = entry.identifier.clone();

            match result {
                Ok(()) => {
                    info!(issue_id = %issue_id, identifier = %identifier, "Worker finished successfully");
                    // Normal exit -> short continuation delay, then retry if issue still active
                    let tx = self.tx.clone();
                    let id = issue_id.clone();
                    let timer_handle = tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(1_000)).await;
                        let _ = tx.send(OrchestratorMsg::RetryIssue { issue_id: id });
                    });

                    let attempt = state.retry_attempts.get(&issue_id)
                        .map(|r| r.attempt)
                        .unwrap_or(0)
                        + 1;

                    state.retry_attempts.insert(issue_id.clone(), RetryEntry {
                        attempt,
                        due_at: std::time::Instant::now() + Duration::from_millis(1_000),
                        timer_handle,
                        identifier: Some(identifier),
                        error: None,
                    });
                }
                Err(e) => {
                    warn!(issue_id = %issue_id, identifier = %identifier, error = %e, "Worker failed");

                    let attempt = state.retry_attempts.get(&issue_id)
                        .map(|r| r.attempt + 1)
                        .unwrap_or(1);
                    let backoff_ms = compute_backoff(
                        ExitType::Failure,
                        attempt,
                        self.config.agent.max_retry_backoff_ms,
                    );

                    let tx = self.tx.clone();
                    let id = issue_id.clone();
                    let timer_handle = tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                        let _ = tx.send(OrchestratorMsg::RetryIssue { issue_id: id });
                    });

                    state.retry_attempts.insert(issue_id.clone(), RetryEntry {
                        attempt,
                        due_at: std::time::Instant::now() + Duration::from_millis(backoff_ms),
                        timer_handle,
                        identifier: Some(identifier),
                        error: Some(e.to_string()),
                    });
                }
            }
        }
    }

    fn handle_agent_update(&self, state: &mut OrchestratorState, issue_id: String, update: AgentUpdate) {
        if let Some(entry) = state.running.get_mut(&issue_id) {
            entry.last_event_timestamp = Some(chrono::Utc::now());

            match update {
                AgentUpdate::Event { event_type, message, input_tokens, output_tokens } => {
                    entry.last_event = Some(event_type);
                    entry.last_event_message = message;
                    entry.input_tokens += input_tokens;
                    entry.output_tokens += output_tokens;
                    entry.total_tokens = entry.input_tokens + entry.output_tokens;
                }
                AgentUpdate::TurnComplete { .. } => {
                    entry.turn_count += 1;
                }
                _ => {}
            }
        }
    }

    async fn handle_retry(&self, state: &mut OrchestratorState, issue_id: String) {
        // Remove from retry queue
        if state.retry_attempts.remove(&issue_id).is_some() {
            // Re-fetch issue and dispatch if still active
            match self.tracker.fetch_issues_by_ids(&[issue_id.clone()]).await {
                Ok(issues) => {
                    if let Some(issue) = issues.into_iter().next() {
                        if issue.is_active() && state.running.len() < state.max_concurrent_agents {
                            self.dispatch_issue(state, issue).await;
                        } else {
                            // Issue no longer active or no slots
                            state.claimed.remove(&issue_id);
                        }
                    } else {
                        // Issue not found
                        state.claimed.remove(&issue_id);
                    }
                }
                Err(e) => {
                    warn!(issue_id = %issue_id, error = %e, "Failed to fetch issue for retry");
                    state.claimed.remove(&issue_id);
                }
            }
        }
    }

    async fn reconcile(&self, state: &mut OrchestratorState, candidates: &[Issue]) {
        let candidate_ids: HashSet<String> = candidates.iter().map(|i| i.id.clone()).collect();

        // Check each running issue for stalls or removal from candidates
        let to_stop: Vec<String> = state.running.iter()
            .filter_map(|(id, entry)| {
                // Issue no longer in candidates (closed/moved)
                if !candidate_ids.contains(id) {
                    return Some(id.clone());
                }

                // Check for stall (no activity for stall_timeout_ms)
                if let Some(last_event) = entry.last_event_timestamp {
                    let elapsed = (chrono::Utc::now() - last_event).num_milliseconds() as u64;
                    if elapsed > self.config.claude.stall_timeout_ms {
                        warn!(issue_id = %id, "Stall detected");
                        return Some(id.clone());
                    }
                }

                None
            })
            .collect();

        // Cancel stalled or removed issues
        for id in to_stop {
            if let Some(entry) = state.running.remove(&id) {
                entry.cancel_token.cancel();
                state.claimed.remove(&id);
            }
        }
    }
}
