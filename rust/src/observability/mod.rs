//! Observability - logging and metrics (SPEC §13)

mod metrics;

pub use metrics::{RuntimeSnapshot, RunningEntrySnapshot, RateLimitInfo};

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Initialize logging
pub fn init_logging(log_dir: Option<&std::path::Path>) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_thread_ids(false);

    if let Some(dir) = log_dir {
        // Create file appender
        let file_appender = tracing_appender::rolling::RollingFileAppender::new(
            tracing_appender::rolling::Rotation::DAILY,
            dir,
            "symphony.log",
        );

        let file_layer = tracing_subscriber::fmt::layer()
            .with_target(false)
            .with_thread_ids(false)
            .with_writer(file_appender);

        tracing_subscriber::registry()
            .with(filter)
            .with(fmt_layer)
            .with(file_layer)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt_layer)
            .init();
    }
}

/// Initialize JSON logging
pub fn init_json_logging(log_dir: Option<&std::path::Path>) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_target(false)
        .with_thread_ids(false);

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .init();
}
