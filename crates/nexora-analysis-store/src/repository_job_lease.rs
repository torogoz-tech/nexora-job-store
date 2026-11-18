use nexora_platform::RepositoryJob;
use zeroize::Zeroizing;
/// Internal worker authority. Deliberately neither Debug nor Serialize; callers cannot construct it.
pub struct RepositoryJobLease {
    pub job: RepositoryJob,
    pub fence: i32,
    pub lease_until: String,
    pub(crate) token: Zeroizing<String>,
}
