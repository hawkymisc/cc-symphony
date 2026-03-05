//! Token tracking data structures (SPEC §4.1.6)

use serde::{Deserialize, Serialize};

/// Token usage for a single session/turn
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// Aggregate token totals across all sessions
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub seconds_running: u64,
}

impl TokenTotals {
    /// Create new empty totals
    pub fn new() -> Self {
        Self::default()
    }

    /// Add token usage from a session
    pub fn add(&mut self, usage: &TokenUsage) {
        self.input_tokens += usage.input_tokens;
        self.output_tokens += usage.output_tokens;
        self.total_tokens = self.input_tokens + self.output_tokens;

        if let Some(cache_read) = usage.cache_read_tokens {
            self.cache_read_tokens += cache_read;
        }
        if let Some(cache_creation) = usage.cache_creation_tokens {
            self.cache_creation_tokens += cache_creation;
        }
    }

    /// Add seconds to the running time
    pub fn add_seconds(&mut self, seconds: u64) {
        self.seconds_running += seconds;
    }

    /// Compute delta between current and previously reported values
    /// Returns (input_delta, output_delta, total_delta)
    pub fn compute_delta(
        current_input: u64,
        current_output: u64,
        last_reported_input: u64,
        last_reported_output: u64,
    ) -> (u64, u64, u64) {
        let input_delta = current_input.saturating_sub(last_reported_input);
        let output_delta = current_output.saturating_sub(last_reported_output);
        let total_delta = input_delta + output_delta;
        (input_delta, output_delta, total_delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_usage_total() {
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: None,
            cache_creation_tokens: None,
        };
        assert_eq!(usage.total(), 150);
    }

    #[test]
    fn token_totals_new() {
        let totals = TokenTotals::new();
        assert_eq!(totals.input_tokens, 0);
        assert_eq!(totals.output_tokens, 0);
        assert_eq!(totals.total_tokens, 0);
        assert_eq!(totals.cache_read_tokens, 0);
        assert_eq!(totals.cache_creation_tokens, 0);
        assert_eq!(totals.seconds_running, 0);
    }

    #[test]
    fn token_totals_add() {
        let mut totals = TokenTotals::new();

        totals.add(&TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: Some(20),
            cache_creation_tokens: Some(10),
        });

        assert_eq!(totals.input_tokens, 100);
        assert_eq!(totals.output_tokens, 50);
        assert_eq!(totals.total_tokens, 150);
        assert_eq!(totals.cache_read_tokens, 20);
        assert_eq!(totals.cache_creation_tokens, 10);

        totals.add(&TokenUsage {
            input_tokens: 50,
            output_tokens: 25,
            cache_read_tokens: Some(10),
            cache_creation_tokens: None,
        });

        assert_eq!(totals.input_tokens, 150);
        assert_eq!(totals.output_tokens, 75);
        assert_eq!(totals.total_tokens, 225);
        assert_eq!(totals.cache_read_tokens, 30);
        assert_eq!(totals.cache_creation_tokens, 10);
    }

    #[test]
    fn token_totals_add_seconds() {
        let mut totals = TokenTotals::new();
        totals.add_seconds(30);
        assert_eq!(totals.seconds_running, 30);
        totals.add_seconds(30);
        assert_eq!(totals.seconds_running, 60);
    }

    #[test]
    fn token_totals_compute_delta_basic() {
        let (input, output, total) = TokenTotals::compute_delta(100, 50, 0, 0);
        assert_eq!(input, 100);
        assert_eq!(output, 50);
        assert_eq!(total, 150);
    }

    #[test]
    fn token_totals_compute_delta_incremental() {
        // First call
        let (i1, o1, t1) = TokenTotals::compute_delta(100, 50, 0, 0);
        assert_eq!((i1, o1, t1), (100, 50, 150));

        // Second call (50 more input, 25 more output)
        let (i2, o2, t2) = TokenTotals::compute_delta(150, 75, 100, 50);
        assert_eq!((i2, o2, t2), (50, 25, 75));
    }

    #[test]
    fn token_totals_compute_delta_no_negative() {
        // If reported is less than last (shouldn't happen, but verify saturating_sub)
        let (input, output, total) = TokenTotals::compute_delta(50, 25, 100, 50);
        assert_eq!(input, 0); // saturating_sub prevents underflow
        assert_eq!(output, 0);
        assert_eq!(total, 0);
    }
}
