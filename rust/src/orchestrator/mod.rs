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
use tracing::{info, warn, info_span, Instrument};

use crate::config::AppConfig;
use crate::domain::{Issue, RetryEntry};
use crate::tracker::Tracker;
use crate::agent::{AgentRunner, AgentUpdate};
use crate::observability::RuntimeSnapshot;
use crate::workspace::{prepare_workspace, run_before_run_hook, run_after_run_hook, cleanup_workspace};

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
    /// Workspace was prepared for an issue (path ready for tracking)
    WorkspaceReady {
        issue_id: String,
        path: std::path::PathBuf,
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

    /// Cancel every running agent's cancellation token.
    ///
    /// Called on shutdown so that all in-flight agent tasks receive the signal
    /// and can exit cleanly before the orchestrator loop terminates.
    fn cancel_all_agents(state: &OrchestratorState) {
        for (_, entry) in state.running.iter() {
            entry.cancel_token.cancel();
        }
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
                    Self::cancel_all_agents(&state);
                    break;
                }

                _ = interval.tick() => {
                    // Cancel-safe: abort immediately on shutdown rather than
                    // waiting for the 30 s HTTP timeout.
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => {
                            Self::cancel_all_agents(&state);
                            break;
                        }
                        _ = self.handle_tick(&mut state) => {}
                    }
                }

                Some(msg) = self.rx.recv() => {
                    match msg {
                        OrchestratorMsg::Shutdown => {
                            info!("Shutdown message received");
                            cancel.cancel();
                        }
                        OrchestratorMsg::Tick => {
                            // Cancel-safe for message-triggered ticks.
                            tokio::select! {
                                biased;
                                _ = cancel.cancelled() => {
                                    Self::cancel_all_agents(&state);
                                    break;
                                }
                                _ = self.handle_tick(&mut state) => {}
                            }
                        }
                        OrchestratorMsg::WorkerFinished { issue_id, result } => {
                            self.handle_worker_finished(&mut state, issue_id, result).await;
                        }
                        OrchestratorMsg::WorkspaceReady { issue_id, path } => {
                            if let Some(entry) = state.running.get_mut(&issue_id) {
                                entry.workspace_path = Some(path);
                            }
                        }
                        OrchestratorMsg::AgentUpdate { issue_id, update } => {
                            self.handle_agent_update(&mut state, issue_id, update);
                        }
                        OrchestratorMsg::RetryIssue { issue_id } => {
                            // Cancel-safe: handle_retry makes a network call.
                            tokio::select! {
                                biased;
                                _ = cancel.cancelled() => {
                                    Self::cancel_all_agents(&state);
                                    break;
                                }
                                _ = self.handle_retry(&mut state, issue_id) => {}
                            }
                        }
                        OrchestratorMsg::ConfigReloaded => {
                            info!("Config reloaded");
                        }
                        OrchestratorMsg::SnapshotRequest { reply } => {
                            let snapshot = state.to_snapshot();
                            let _ = reply.send(snapshot);
                        }
                        OrchestratorMsg::RefreshRequest { reply } => {
                            // Cancel-safe: handle_tick makes a network call.
                            tokio::select! {
                                biased;
                                _ = cancel.cancelled() => {
                                    Self::cancel_all_agents(&state);
                                    let _ = reply.send(());
                                    break;
                                }
                                _ = self.handle_tick(&mut state) => {}
                            }
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
                    self.dispatch_issue(state, issue, 0).await;
                }

                // Reconcile running issues
                self.reconcile(state, &candidates).await;
            }
            Err(e) => {
                warn!("Failed to fetch candidates: {}", e);
            }
        }
    }

    async fn dispatch_issue(&self, state: &mut OrchestratorState, issue: Issue, consecutive_failures: u32) {
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
        let attempt = if consecutive_failures > 0 { Some(consecutive_failures) } else { None };

        let (update_tx, mut update_rx) = mpsc::unbounded_channel::<(String, AgentUpdate)>();

        // Forward agent updates to the orchestrator channel
        let tx_forward = tx.clone();
        tokio::spawn(async move {
            while let Some((id, update)) = update_rx.recv().await {
                let _ = tx_forward.send(OrchestratorMsg::AgentUpdate { issue_id: id, update });
            }
        });

        let tx_finish = tx.clone();
        let hooks = config.hooks.clone();

        let span = info_span!(
            "issue",
            issue_id = %issue_id,
            identifier = %identifier,
        );

        let task_handle = tokio::spawn(async move {
            // Prepare workspace (creates dir + runs after_create hook on first use)
            let workspace_path = match prepare_workspace(&config.workspace, &hooks, &issue_clone).await {
                Ok(p) => {
                    // Notify the orchestrator of the workspace path so it can be tracked for cleanup
                    let _ = tx_finish.send(OrchestratorMsg::WorkspaceReady {
                        issue_id: issue_clone.id.clone(),
                        path: p.path.clone(),
                    });
                    p.path
                }
                Err(e) => {
                    warn!("prepare_workspace failed for issue {}: {}", issue_clone.identifier, e);
                    let _ = tx_finish.send(OrchestratorMsg::WorkerFinished {
                        issue_id: issue_clone.id.clone(),
                        result: Err(crate::agent::AgentError::SpawnFailed(e.to_string())),
                    });
                    return;
                }
            };

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
        }.instrument(span));

        let entry = RunningEntry {
            task_handle: Some(task_handle),
            identifier,
            issue,
            cancel_token,
            consecutive_failures,
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
            let workspace_path = entry.workspace_path.clone();
            let elapsed_secs = (chrono::Utc::now() - entry.started_at).num_milliseconds().max(0) as u64 / 1000;
            state.agent_totals.add_seconds(elapsed_secs);

            match result {
                Ok(()) => {
                    info!(issue_id = %issue_id, identifier = %identifier, "Worker finished successfully");
                    // Record this issue as having had at least one successful turn.
                    state.completed_count += 1;
                    // Normal exit -> 1s continuation delay. Reset consecutive failure count.
                    let tx = self.tx.clone();
                    let id = issue_id.clone();
                    let timer_handle = tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(1_000)).await;
                        let _ = tx.send(OrchestratorMsg::RetryIssue { issue_id: id });
                    });

                    // attempt = 0: success resets consecutive failure counter
                    state.retry_attempts.insert(issue_id.clone(), RetryEntry {
                        attempt: 0,
                        due_at: std::time::Instant::now() + Duration::from_millis(1_000),
                        timer_handle,
                        identifier: Some(identifier),
                        error: None,
                        workspace_path,
                    });
                }
                Err(e) => {
                    warn!(issue_id = %issue_id, identifier = %identifier, error = %e, "Worker failed");

                    // Increment consecutive failure count carried from RunningEntry.
                    // (retry_attempts entry was removed by handle_retry before this run,
                    //  so we read it from RunningEntry which survives dispatch.)
                    let failure_count = entry.consecutive_failures + 1;
                    let backoff_ms = compute_backoff(
                        ExitType::Failure,
                        failure_count,
                        self.config.agent.max_retry_backoff_ms,
                    );

                    let tx = self.tx.clone();
                    let id = issue_id.clone();
                    let timer_handle = tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                        let _ = tx.send(OrchestratorMsg::RetryIssue { issue_id: id });
                    });

                    state.retry_attempts.insert(issue_id.clone(), RetryEntry {
                        attempt: failure_count,
                        due_at: std::time::Instant::now() + Duration::from_millis(backoff_ms),
                        timer_handle,
                        identifier: Some(identifier),
                        error: Some(e.to_string()),
                        workspace_path,
                    });
                }
            }
        }
    }

    fn handle_agent_update(&self, state: &mut OrchestratorState, issue_id: String, update: AgentUpdate) {
        // Extract token deltas before borrowing entry mutably, so we can
        // update both the entry and agent_totals without borrow conflicts.
        let token_delta = if let Some(entry) = state.running.get_mut(&issue_id) {
            entry.last_event_timestamp = Some(chrono::Utc::now());

            match update {
                AgentUpdate::Event { event_type, message, input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens } => {
                    entry.last_event = Some(event_type);
                    entry.last_event_message = message;
                    entry.input_tokens += input_tokens;
                    entry.output_tokens += output_tokens;
                    entry.total_tokens = entry.input_tokens + entry.output_tokens;
                    Some((input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens))
                }
                AgentUpdate::Started { session_id } => {
                    entry.session_id = Some(session_id);
                    None
                }
                AgentUpdate::TurnComplete { .. } => {
                    entry.turn_count += 1;
                    None
                }
                _ => None,
            }
        } else {
            None
        };

        // Accumulate deltas into aggregate totals (borrow of entry is dropped above)
        if let Some((input_delta, output_delta, cache_creation, cache_read)) = token_delta {
            state.agent_totals.add(&crate::domain::TokenUsage {
                input_tokens: input_delta,
                output_tokens: output_delta,
                cache_read_tokens: if cache_read > 0 { Some(cache_read) } else { None },
                cache_creation_tokens: if cache_creation > 0 { Some(cache_creation) } else { None },
            });
        }
    }

    async fn handle_retry(&self, state: &mut OrchestratorState, issue_id: String) {
        // Remove from retry queue and capture metadata before dispatch
        if let Some(removed_entry) = state.retry_attempts.remove(&issue_id) {
            let prior_failures = if removed_entry.error.is_some() { removed_entry.attempt } else { 0 };
            // Preserve original attempt count and metadata for potential re-queue
            let entry_attempt = removed_entry.attempt;
            let entry_identifier = removed_entry.identifier.clone();
            let entry_error = removed_entry.error.clone();
            let workspace_path = removed_entry.workspace_path.clone();

            // Re-fetch issue and dispatch if still active
            match self.tracker.fetch_issues_by_ids(std::slice::from_ref(&issue_id)).await {
                Ok(issues) => {
                    match issues.into_iter().next() {
                        Some(issue) if issue.is_active() => {
                            if state.running.len() < state.max_concurrent_agents {
                                self.dispatch_issue(state, issue, prior_failures).await;
                            } else {
                                // Issue is still open but no slots available right now.
                                // Re-queue with a short delay so we try again when slots free up.
                                // Preserve workspace_path and failure count — do NOT release claim.
                                let tx = self.tx.clone();
                                let id = issue_id.clone();
                                const SLOT_WAIT_MS: u64 = 1_000;
                                let timer_handle = tokio::spawn(async move {
                                    tokio::time::sleep(Duration::from_millis(SLOT_WAIT_MS)).await;
                                    let _ = tx.send(OrchestratorMsg::RetryIssue { issue_id: id });
                                });
                                state.retry_attempts.insert(issue_id.clone(), RetryEntry {
                                    attempt: entry_attempt,
                                    due_at: std::time::Instant::now() + Duration::from_millis(SLOT_WAIT_MS),
                                    timer_handle,
                                    identifier: entry_identifier,
                                    error: entry_error,
                                    workspace_path,
                                });
                            }
                        }
                        _ => {
                            // Issue not found or terminal — abandon claim and clean up workspace
                            state.claimed.remove(&issue_id);
                            if let Some(path) = workspace_path {
                                let hooks = self.config.hooks.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = cleanup_workspace(&path, &hooks).await {
                                        warn!(error = %e, "cleanup_workspace failed (non-fatal)");
                                    }
                                });
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(issue_id = %issue_id, error = %e, "Failed to fetch issue for retry; rescheduling");
                    // Transient tracker error — reschedule retry with same failure count
                    // so exponential backoff is preserved across tracker outages.
                    let tx = self.tx.clone();
                    let id = issue_id.clone();
                    let backoff_ms = compute_backoff(
                        ExitType::Failure,
                        prior_failures.max(1),
                        self.config.agent.max_retry_backoff_ms,
                    );
                    let timer_handle = tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                        let _ = tx.send(OrchestratorMsg::RetryIssue { issue_id: id });
                    });
                    state.retry_attempts.insert(issue_id.clone(), crate::domain::RetryEntry {
                        attempt: prior_failures,
                        due_at: std::time::Instant::now() + Duration::from_millis(backoff_ms),
                        timer_handle,
                        identifier: None,
                        error: Some(e.to_string()),
                        workspace_path,
                    });
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
