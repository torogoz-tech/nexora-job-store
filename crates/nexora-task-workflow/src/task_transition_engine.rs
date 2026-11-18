pub(crate) struct TaskTransitionEngine;

#[path = "task_transition_engine_approval.rs"]
mod task_transition_engine_approval;
#[path = "task_transition_engine_execution.rs"]
mod task_transition_engine_execution;
#[path = "task_transition_engine_notify.rs"]
mod task_transition_engine_notify;
#[path = "task_transition_engine_preflight.rs"]
mod task_transition_engine_preflight;
#[path = "task_transition_engine_provider_steps.rs"]
mod task_transition_engine_provider_steps;
#[path = "task_transition_engine_resume.rs"]
mod task_transition_engine_resume;
