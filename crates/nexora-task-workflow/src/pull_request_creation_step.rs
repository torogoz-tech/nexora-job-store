pub(crate) struct PullRequestCreationStep<'a> {
    pub(crate) repository: Option<&'a str>,
    pub(crate) literal_source: &'a str,
    pub(crate) literal_target: &'a str,
    pub(crate) from_task: bool,
    pub(crate) from_parent: bool,
    pub(crate) index: usize,
    pub(crate) revalidate: bool,
}
