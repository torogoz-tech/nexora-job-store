use nexora_platform::TaskTransitionBranch;

pub(crate) struct ProviderObservationRequest<'a> {
    pub(crate) branch: &'a TaskTransitionBranch,
    pub(crate) observation: &'a str,
    pub(crate) selector: Option<&'a str>,
    pub(crate) target_branch: Option<&'a str>,
}
