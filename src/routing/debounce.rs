use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// The channel ID and the author ID.
pub type Key = (u64, u64);

type PendingMap<T> = Arc<Mutex<HashMap<Key, Pending<T>>>>;

/// The items of one key that wait for the end of the quiet period, and the timer of that
/// period.
struct Pending<T> {
    items: Vec<T>,
    timer: JoinHandle<()>,
}

/// The debouncer that collects the items of each key. When a key gets no item for the quiet
/// period, the debouncer sends the items of the key to the output as one unit.
pub struct Debouncer<T> {
    quiet: Duration,
    output: mpsc::UnboundedSender<Vec<T>>,
    pending: PendingMap<T>,
}

impl<T: Send + 'static> Debouncer<T> {
    pub fn new(quiet: Duration, output: mpsc::UnboundedSender<Vec<T>>) -> Debouncer<T> {
        Debouncer {
            quiet,
            output,
            pending: Arc::default(),
        }
    }

    pub fn push(&self, key: Key, item: T) {
        let mut pending = self.pending.lock().unwrap();
        let mut items = match pending.remove(&key) {
            Some(previous) => {
                previous.timer.abort();
                previous.items
            }
            None => Vec::new(),
        };
        items.push(item);
        let timer = self.start_timer(key);
        pending.insert(key, Pending { items, timer });
    }

    fn start_timer(&self, key: Key) -> JoinHandle<()> {
        let quiet = self.quiet;
        let output = self.output.clone();
        let pending = Arc::clone(&self.pending);
        tokio::spawn(async move {
            tokio::time::sleep(quiet).await;
            if let Some(items) = take_items_of_current_timer(&pending, key) {
                // The receiver closes only when the bot stops. In that case, the unit has no
                // destination.
                let _ = output.send(items);
            }
        })
    }
}

/// If the calling task is the current timer of the key, removes the key and returns its items.
/// `push` cannot abort a timer that ended while `push` held the lock. That timer must not take
/// the items that `push` gave to the next timer.
fn take_items_of_current_timer<T>(pending: &PendingMap<T>, key: Key) -> Option<Vec<T>> {
    let mut pending = pending.lock().unwrap();
    let is_current = pending
        .get(&key)
        .is_some_and(|entry| entry.timer.id() == tokio::task::id());
    if !is_current {
        return None;
    }
    pending.remove(&key).map(|entry| entry.items)
}

#[cfg(test)]
mod tests {
    use tokio::time::Instant;

    use super::*;

    const QUIET: Duration = Duration::from_secs(3);
    const ALICE_IN_GENERAL: Key = (1, 10);

    fn debouncer() -> (
        Debouncer<&'static str>,
        mpsc::UnboundedReceiver<Vec<&'static str>>,
    ) {
        let (output, units) = mpsc::unbounded_channel();
        (Debouncer::new(QUIET, output), units)
    }

    #[tokio::test(start_paused = true)]
    async fn sends_all_items_after_the_quiet_period() {
        let (debouncer, mut units) = debouncer();
        let start = Instant::now();

        debouncer.push(ALICE_IN_GENERAL, "hi");
        debouncer.push(ALICE_IN_GENERAL, "nuclear crashes");
        let unit = units.recv().await.unwrap();

        assert_eq!(unit, vec!["hi", "nuclear crashes"]);
        assert_eq!(start.elapsed(), QUIET);
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_item_restarts_the_timer() {
        let (debouncer, mut units) = debouncer();
        let start = Instant::now();

        debouncer.push(ALICE_IN_GENERAL, "hi");
        tokio::time::sleep(Duration::from_secs(2)).await;
        debouncer.push(ALICE_IN_GENERAL, "nuclear crashes");
        let unit = units.recv().await.unwrap();

        assert_eq!(unit, vec!["hi", "nuclear crashes"]);
        assert_eq!(start.elapsed(), Duration::from_secs(5));
    }

    #[tokio::test(start_paused = true)]
    async fn different_keys_make_different_units() {
        let (debouncer, mut units) = debouncer();

        debouncer.push(ALICE_IN_GENERAL, "hi");
        tokio::time::sleep(Duration::from_secs(1)).await;
        let bob_in_general = (1, 20);
        debouncer.push(bob_in_general, "hello");
        let first = units.recv().await.unwrap();
        let second = units.recv().await.unwrap();

        assert_eq!(first, vec!["hi"]);
        assert_eq!(second, vec!["hello"]);
    }
}
