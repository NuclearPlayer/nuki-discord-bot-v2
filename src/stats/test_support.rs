use crate::stats::recorder::{RunStats, Stats};
use crate::store::Store;

/// A `RunStats` on an in-memory store.
pub(crate) async fn run_stats(run_id: &str, guild_id: u64, channel_id: u64) -> (Store, RunStats) {
    let store = Store::open_in_memory().await.unwrap();
    let run = Stats::new(store.events()).run(run_id.to_string(), guild_id, channel_id);
    (store, run)
}
