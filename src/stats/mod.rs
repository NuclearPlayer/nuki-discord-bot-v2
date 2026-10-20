pub mod event;
pub mod recorder;
#[cfg(test)]
pub(crate) mod test_support;

use std::time::Duration;

/// Converts `elapsed` to the `duration_ms` of an event.
pub fn duration_ms(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}
