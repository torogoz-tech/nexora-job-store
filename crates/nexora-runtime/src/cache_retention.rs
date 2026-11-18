pub struct CacheRetention;
impl CacheRetention {
    /// Retention is independent of validity: identities determine freshness.
    pub fn seconds(computation_us: u64, bytes: usize) -> u64 {
        let megabytes = u64::try_from(bytes / 1_048_576).unwrap_or(u64::MAX);
        computation_us
            .saturating_div(1000)
            .saturating_mul(60)
            .saturating_div(megabytes.saturating_add(1))
            .clamp(300, 86_400)
    }
}
