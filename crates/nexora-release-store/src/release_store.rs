use nexora_analysis_store::AnalysisStore;
use nexora_database_connections::PostgresConnection;
use nexora_platform::PlatformError;

pub struct ReleaseStore {
    pub(crate) client: PostgresConnection,
    pub(crate) provider_store: AnalysisStore,
}

impl ReleaseStore {
    pub(crate) fn database_error(error: postgres::Error) -> PlatformError {
        if error.code().is_some_and(|code| {
            code == &postgres::error::SqlState::T_R_SERIALIZATION_FAILURE
                || code == &postgres::error::SqlState::T_R_DEADLOCK_DETECTED
        }) {
            PlatformError::Conflict(
                "release composition conflicted with another database operation",
            )
        } else {
            PlatformError::Storage
        }
    }
}
