use std::io;

#[cfg(any(unix, windows))]
pub(crate) fn nonce() -> io::Result<u128> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random)
        .map_err(|error| io::Error::other(format!("failed to generate temporary name: {error}")))?;
    Ok(u128::from_ne_bytes(random))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn nonce() -> io::Result<u128> {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(timestamp ^ u128::from(counter))
}
