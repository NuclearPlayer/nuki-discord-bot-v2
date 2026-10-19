use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The unsolicited chat cooldown of each channel.
pub struct Cooldowns {
    length: Duration,
    starts: Mutex<HashMap<u64, Instant>>,
}

impl Cooldowns {
    pub fn new(length: Duration) -> Cooldowns {
        Cooldowns {
            length,
            starts: Mutex::new(HashMap::new()),
        }
    }

    pub fn is_active(&self, channel_id: u64, now: Instant) -> bool {
        self.starts
            .lock()
            .unwrap()
            .get(&channel_id)
            .is_some_and(|start| now.saturating_duration_since(*start) < self.length)
    }

    pub fn start(&self, channel_id: u64, now: Instant) {
        self.starts.lock().unwrap().insert(channel_id, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LENGTH: Duration = Duration::from_secs(15 * 60);
    const GENERAL: u64 = 1;
    const SUPPORT: u64 = 2;

    #[test]
    fn is_active_after_start() {
        let cooldowns = Cooldowns::new(LENGTH);
        let start = Instant::now();

        cooldowns.start(GENERAL, start);

        assert!(cooldowns.is_active(GENERAL, start + LENGTH - Duration::from_secs(1)));
    }

    #[test]
    fn is_not_active_after_the_length() {
        let cooldowns = Cooldowns::new(LENGTH);
        let start = Instant::now();

        cooldowns.start(GENERAL, start);

        assert!(!cooldowns.is_active(GENERAL, start + LENGTH));
    }

    #[test]
    fn is_per_channel() {
        let cooldowns = Cooldowns::new(LENGTH);
        let start = Instant::now();

        cooldowns.start(GENERAL, start);

        let active = [GENERAL, SUPPORT].map(|channel_id| cooldowns.is_active(channel_id, start));

        assert_eq!(active, [true, false]);
    }
}
