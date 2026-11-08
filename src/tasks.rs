//! The background task: the periodic fetch of the clones.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::{Instant, MissedTickBehavior, interval_at};

use crate::repos::manager::RepoManager;

/// Fetches the clones at intervals of `period`. The first fetch occurs after one period,
/// because `App::build` clones at start.
pub fn spawn_repo_fetch(repos: Arc<RepoManager>, period: Duration) {
    tokio::spawn(async move {
        let mut ticks = interval_at(Instant::now() + period, period);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            for (id, error) in repos.fetch_all().await {
                tracing::warn!(repo = %id, "could not fetch the repository: {error}");
            }
        }
    });
}
