use crate::{
    DailyDeveloperAnalysisJob, KnowledgeStore, knowledge_store_secret::KnowledgeStoreSecret,
};
use nexora_platform::PlatformError;
use serde_json::Value;

impl KnowledgeStore {
    pub fn schedule_daily_developer_analyses(&mut self) -> Result<u64, PlatformError> {
        self.client()
            .execute(
                "INSERT INTO daily_developer_analyses(actor_id,date,timezone) SELECT a.id,d::date,s.timezone FROM actors a JOIN actor_roles r ON r.actor_id=a.id CROSS JOIN labor_calendar_settings s CROSS JOIN LATERAL generate_series(GREATEST(coalesce((SELECT max(date)+1 FROM daily_developer_analyses x WHERE x.actor_id=a.id),(clock_timestamp() AT TIME ZONE s.timezone)::date-1),s.starts_on,(a.created_at AT TIME ZONE s.timezone)::date)::timestamp,((clock_timestamp() AT TIME ZONE s.timezone)::date-1)::timestamp,interval '1 day') d WHERE a.active AND a.kind='human' AND r.role IN ('developer','technical_lead') ON CONFLICT(actor_id,date) DO NOTHING",
                &[],
            )
            .map_err(Self::database_error)
    }

    pub fn claim_daily_developer_analysis(
        &mut self,
    ) -> Result<Option<DailyDeveloperAnalysisJob>, PlatformError> {
        let mut transaction = self.client().transaction().map_err(Self::database_error)?;
        transaction
            .execute(
                "UPDATE daily_developer_analyses d SET status='failed' WHERE status='pending' AND NOT EXISTS(SELECT 1 FROM actors a JOIN actor_roles r ON r.actor_id=a.id WHERE a.id=d.actor_id AND a.active AND r.role IN ('developer','technical_lead'))",
                &[],
            )
            .map_err(Self::database_error)?;
        transaction
            .execute(
                "UPDATE daily_developer_analyses SET status='failed',lease_token=NULL WHERE status='running' AND started_at<clock_timestamp()-interval '5 minutes'",
                &[],
            )
            .map_err(Self::database_error)?;
        let row = transaction
            .query_opt(
                "SELECT actor_id,date::text,timezone FROM daily_developer_analyses WHERE status='pending' ORDER BY date,actor_id FOR UPDATE SKIP LOCKED LIMIT 1",
                &[],
            )
            .map_err(Self::database_error)?;
        let Some(row) = row else {
            transaction.commit().map_err(Self::database_error)?;
            return Ok(None);
        };
        let member_id: String = row.get(0);
        let date: String = row.get(1);
        let timezone: String = row.get(2);
        let mut evidence: Value = transaction
            .query_one(
                include_str!("daily_developer_analysis_evidence.sql"),
                &[&member_id, &date, &timezone],
            )
            .map_err(Self::database_error)?
            .get(0);
        evidence["availability"] = transaction
            .query_one(
                "SELECT member_daily_availability($1,$2::text::date)",
                &[&member_id, &date],
            )
            .map_err(Self::database_error)?
            .get(0);
        let has_evidence = evidence["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty());
        let lease_token = KnowledgeStoreSecret::token("daily")?;
        let status = if has_evidence {
            "running"
        } else {
            "no_evidence"
        };
        transaction
            .execute(
                "UPDATE daily_developer_analyses SET status=$3,evidence=$4,lease_token=$5,started_at=clock_timestamp(),generated_at=CASE WHEN $3='no_evidence' THEN clock_timestamp() ELSE NULL END WHERE actor_id=$1 AND date=$2::text::date AND status='pending'",
                &[&member_id, &date, &status, &evidence, &lease_token],
            )
            .map_err(Self::database_error)?;
        let language: String = transaction
            .query_one("SELECT language FROM installation_preferences LIMIT 1", &[])
            .map_err(Self::database_error)?
            .get(0);
        transaction.commit().map_err(Self::database_error)?;
        if !has_evidence {
            return Ok(None);
        }
        Ok(Some(DailyDeveloperAnalysisJob {
            member_id,
            date,
            timezone,
            lease_token,
            evidence,
            language,
        }))
    }

    pub fn finish_daily_developer_analysis(
        &mut self,
        job: &DailyDeveloperAnalysisJob,
        report: Option<&Value>,
    ) -> Result<bool, PlatformError> {
        let status = if report.is_some() {
            "complete"
        } else {
            "failed"
        };
        self.client()
            .execute(
                "UPDATE daily_developer_analyses SET status=$4,report=$5,generated_at=clock_timestamp(),lease_token=NULL WHERE actor_id=$1 AND date=$2::text::date AND status='running' AND lease_token=$3",
                &[&job.member_id, &job.date, &job.lease_token, &status, &report],
            )
            .map(|changed| changed == 1)
            .map_err(Self::database_error)
    }
}
