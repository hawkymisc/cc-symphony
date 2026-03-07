//! Symphony CLI entrypoint

use clap::Parser;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use symphony::agent::ClaudeRunner;
use symphony::config::AppConfig;
use symphony::orchestrator::Orchestrator;
use symphony::tracker::{GitHubConfig, GitHubTracker};
use symphony::load_workflow;

/// Symphony - Issue tracker to coding agent orchestrator
#[derive(Parser, Debug)]
#[command(name = "symphony")]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to WORKFLOW.md file
    #[arg(default_value = "./WORKFLOW.md")]
    workflow_path: PathBuf,

    /// Enable HTTP server on this port
    #[arg(short, long)]
    port: Option<u16>,

    /// Increase log verbosity (repeat for more)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Suppress non-error output
    #[arg(short, long)]
    quiet: bool,

    /// Validate config and exit without starting
    #[arg(long)]
    dry_run: bool,
}

mod exit_codes {
    pub const SUCCESS: i32 = 0;
    pub const CONFIG_ERROR: i32 = 2;
    pub const WORKFLOW_ERROR: i32 = 3;
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // Initialize logging
    let level = if args.quiet {
        tracing::Level::ERROR
    } else if args.verbose >= 2 {
        tracing::Level::TRACE
    } else if args.verbose == 1 {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };
    tracing_subscriber::fmt().with_max_level(level).init();

    // Load workflow
    let workflow = match load_workflow(&args.workflow_path) {
        Ok(w) => w,
        Err(e) => {
            error!("Failed to load workflow: {}", e);
            std::process::exit(exit_codes::WORKFLOW_ERROR);
        }
    };

    // Parse and validate config
    let config = match AppConfig::from_workflow(&workflow) {
        Ok(c) => c,
        Err(e) => {
            error!("Config error: {}", e);
            std::process::exit(exit_codes::CONFIG_ERROR);
        }
    };

    if let Err(e) = config.validate() {
        error!("Config validation error: {}", e);
        std::process::exit(exit_codes::CONFIG_ERROR);
    }

    // Dry run — just validate and exit
    if args.dry_run {
        println!("Config validated successfully");
        println!("  Tracker: {} ({})", config.tracker.kind, config.tracker.repo.as_deref().unwrap_or("N/A"));
        println!("  Model: {}", config.claude.model);
        println!("  Max concurrent agents: {}", config.agent.max_concurrent_agents);
        println!("  Poll interval: {}ms", config.polling.interval_ms);
        std::process::exit(exit_codes::SUCCESS);
    }

    // Print startup info
    info!("symphony v{} starting", env!("CARGO_PKG_VERSION"));
    info!("workflow: {}", args.workflow_path.display());
    info!("tracker: {} ({})", config.tracker.kind, config.tracker.repo.as_deref().unwrap_or("N/A"));
    info!("agent: {} ({})", config.claude.command, config.claude.model);
    info!("workspace root: {}", config.workspace.root.display());
    info!("concurrency: max {} agents", config.agent.max_concurrent_agents);
    info!("polling every {}ms", config.polling.interval_ms);

    // Build tracker
    let github_config = GitHubConfig {
        endpoint: config.tracker.endpoint.clone(),
        api_key: config.tracker.api_key.clone().unwrap_or_default(),
        repo: config.tracker.repo.clone().unwrap_or_default(),
        labels: config.tracker.labels.clone(),
        active_states: config.tracker.active_states.clone(),
        terminal_states: config.tracker.terminal_states.clone(),
    };
    let tracker = match GitHubTracker::new(github_config) {
        Ok(t) => t,
        Err(e) => {
            error!("Failed to create tracker: {}", e);
            std::process::exit(exit_codes::CONFIG_ERROR);
        }
    };

    // Build agent runner
    let agent_runner = ClaudeRunner;

    // Set up cancellation token for graceful shutdown
    let cancel = CancellationToken::new();

    // Register signal handlers (SIGTERM + SIGINT)
    let cancel_signals = cancel.clone();
    tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();
        #[cfg(unix)]
        let mut sigterm = {
            use tokio::signal::unix::{signal, SignalKind};
            signal(SignalKind::terminate()).expect("Failed to register SIGTERM handler")
        };

        #[cfg(unix)]
        tokio::select! {
            _ = ctrl_c => { info!("Received SIGINT, shutting down..."); }
            _ = sigterm.recv() => { info!("Received SIGTERM, shutting down..."); }
        }

        #[cfg(not(unix))]
        ctrl_c.await.ok();

        cancel_signals.cancel();
    });

    // Build and run orchestrator
    let (orchestrator, tx) = Orchestrator::new(tracker, agent_runner, config);

    // Start optional HTTP server
    #[cfg(feature = "http-server")]
    if let Some(port) = args.port {
        let tx_http = tx.clone();
        let cancel_http = cancel.clone();
        let listener = match tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await {
            Ok(l) => l,
            Err(e) => {
                error!("Failed to bind HTTP server on port {}: {}", port, e);
                std::process::exit(exit_codes::CONFIG_ERROR);
            }
        };
        info!("HTTP server listening on port {}", port);
        tokio::spawn(async move {
            if let Err(e) = symphony::http_server::start_server(listener, tx_http, cancel_http).await {
                tracing::warn!("HTTP server error: {}", e);
            }
        });
    }

    orchestrator.run(cancel).await;

    info!("Symphony stopped");
    std::process::exit(exit_codes::SUCCESS);
}
