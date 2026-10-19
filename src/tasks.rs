//! The background tasks: the periodic fetch of the clones, and the intake timeout.

use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeDelta, Utc};
use tokio::time::{Instant, MissedTickBehavior, interval, interval_at};

use crate::app::App;
use crate::discord::intake_thread::complete_inactive_intake;
use crate::repos::manager::RepoManager;
use crate::workflows::intake::timeout::due_intakes;

const INTAKE_CHECK_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// Fetches the clones every `period`. The first fetch is after one period, because
/// `App::build` clones at start.
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

/// Completes the intakes whose reporter sent no message for `INTAKE_TIMEOUT_HOURS`.
pub fn spawn_intake_timeout(app: Arc<App>) {
    let timeout = TimeDelta::hours(app.config.intake_timeout_hours as i64);
    tokio::spawn(async move {
        let mut ticks = interval(INTAKE_CHECK_INTERVAL);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            match due_intakes(&app.store.intakes(), Utc::now(), timeout).await {
                Ok(intakes) => {
                    for intake in intakes {
                        complete_inactive_intake(&app, &intake).await;
                    }
                }
                Err(error) => tracing::error!("could not get the due intakes: {error}"),
            }
        }
    });
}
