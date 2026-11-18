use serde_json::Value;

pub(crate) struct TaskContributorRatingCompletion<'a> {
    pub(crate) id: &'a str,
    pub(crate) claim_token: &'a str,
    pub(crate) review: &'a str,
    pub(crate) actor: &'a str,
    pub(crate) project: &'a str,
    pub(crate) input: &'a Value,
    pub(crate) report: &'a Value,
    pub(crate) model: &'a str,
}
