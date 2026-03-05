//! Symphony CLI entrypoint

use clap::Parser;
use std::path::PathBuf;

use symphony::{load_workflow, AppConfig};

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

    /// Run once and exit (no polling loop)
    #[arg(long)]
    once: bool,
}

/// Exit codes
mod exit_codes {
    pub const SUCCESS: i32 = 0;
    pub const ERROR: i32 = 1;
    pub const CONFIG_ERROR: i32 = 2;
    pub const INTERRUPTED: i32 = 3;
}

fn main() {
    let args = Args::parse();

    // Initialize logging
    if args.quiet {
        // Only errors
    } else if args.verbose > 0 {
        // More verbose
    }

    // Load workflow
    let workflow = match load_workflow(&args.workflow_path) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Error loading workflow: {}", e);
            std::process::exit(exit_codes::CONFIG_ERROR);
        }
    };

    // Parse config
    let config = match AppConfig::from_workflow(&workflow) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Config error: {}", e);
            std::process::exit(exit_codes::CONFIG_ERROR);
        }
    };

    // Validate config
    if let Err(e) = config.validate() {
        eprintln!("Config validation error: {}", e);
        std::process::exit(exit_codes::CONFIG_ERROR);
    }

    // Dry run - just validate and exit
    if args.dry_run {
        println!("Config validated successfully");
        println!("  Tracker: {} ({})", config.tracker.kind, config.tracker.repo.as_deref().unwrap_or("N/A"));
        println!("  Model: {}", config.claude.model);
        println!("  Max concurrent agents: {}", config.agent.max_concurrent_agents);
        println!("  Poll interval: {}ms", config.polling.interval_ms);
        std::process::exit(exit_codes::SUCCESS);
    }

    // Run orchestrator
    // For now, just print startup info
    println!("Symphony starting...");
    println!("  Workflow: {}", args.workflow_path.display());
    println!("  Tracker: {} ({})", config.tracker.kind, config.tracker.repo.as_deref().unwrap_or("N/A"));
    println!("  Agent: {} ({})", config.claude.command, config.claude.model);
    println!("  Workspace root: {}", config.workspace.root.display());
    println!("  Concurrency: max {} agents", config.agent.max_concurrent_agents);
    println!("  Polling every {}ms", config.polling.interval_ms);

    // In a full implementation, we would:
    // 1. Create tracker instance
    // 2. Create agent runner instance
    // 3. Create orchestrator
    // 4. Set up signal handlers
    // 5. Run event loop

    // For now, just exit successfully
    std::process::exit(exit_codes::SUCCESS);
}
