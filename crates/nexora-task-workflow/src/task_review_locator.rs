pub(crate) struct TaskReviewLocator<'a> {
    pub(crate) project: &'a str,
    pub(crate) task: &'a str,
    pub(crate) revision: i64,
    pub(crate) repository: &'a str,
    pub(crate) head: &'a str,
    pub(crate) review_id: Option<&'a str>,
}
