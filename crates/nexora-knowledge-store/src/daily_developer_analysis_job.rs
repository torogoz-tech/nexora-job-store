use serde_json::Value;

pub struct DailyDeveloperAnalysisJob {
    pub member_id: String,
    pub date: String,
    pub timezone: String,
    pub lease_token: String,
    pub evidence: Value,
    pub language: String,
}
