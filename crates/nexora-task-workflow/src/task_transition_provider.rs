pub(crate) struct TaskTransitionProvider;

#[path = "task_transition_provider_branch.rs"]
mod task_transition_provider_branch;
#[path = "task_transition_provider_merge.rs"]
mod task_transition_provider_merge;
#[path = "task_transition_provider_observe.rs"]
mod task_transition_provider_observe;
#[path = "task_transition_provider_pull_request.rs"]
mod task_transition_provider_pull_request;
#[path = "task_transition_provider_release.rs"]
mod task_transition_provider_release;
#[path = "task_transition_provider_review.rs"]
mod task_transition_provider_review;
#[path = "task_transition_provider_source.rs"]
mod task_transition_provider_source;
#[path = "task_transition_provider_verify.rs"]
mod task_transition_provider_verify;
