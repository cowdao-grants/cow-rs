use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) fn valid_to_after(seconds: u64) -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
        .saturating_add(seconds)
        .min(u64::from(u32::MAX)) as u32
}
