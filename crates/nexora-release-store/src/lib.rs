mod release_authentication;
mod release_authority;
mod release_composition;
mod release_composition_job;
mod release_store;
mod release_store_compose;
mod release_store_connect;

#[cfg(test)]
#[path = "release_store_connect_tests.rs"]
mod release_store_connect_tests;

#[cfg(test)]
#[path = "release_authority_tests.rs"]
mod release_authority_tests;

#[cfg(test)]
#[path = "release_composition_job_tests.rs"]
mod release_composition_job_tests;

#[cfg(test)]
#[path = "release_composition_tests.rs"]
mod release_composition_tests;

pub use release_store::ReleaseStore;
