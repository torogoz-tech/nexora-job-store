use nexora_platform::PlatformError;
use serde_json::Value;
/// Matches every delivered repository to provider-confirmed merge and deployment facts.
pub(crate) struct TaskDeliveryEvidence;
impl TaskDeliveryEvidence {
    pub fn verify(outcomes: &Value, target: &str, corrective: bool) -> Result<(), PlatformError> {
        let steps = outcomes.as_array().ok_or(PlatformError::Storage)?;
        let merges: Vec<&Value> = steps
            .iter()
            .filter(|s| {
                s["kind"] == "verify_pull_request_merged" || s["kind"] == "merge_pull_request"
            })
            .filter_map(|s| s["result"]["pull_requests"].as_array())
            .flatten()
            .collect();
        let deployments: Vec<&Value> = steps
            .iter()
            .filter(|s| s["kind"] == "verify_deployment")
            .filter_map(|s| s["result"]["deployments"].as_array())
            .flatten()
            .collect();
        let delivered: Vec<_> = merges
            .iter()
            .filter(|m| m["target_branch"] == target && m["provenance"] == "provider_verified")
            .collect();
        if delivered.is_empty() {
            return Err(PlatformError::Conflict(
                "destination requires a provider-confirmed merged pull request",
            ));
        }
        for merged in delivered {
            if !deployments.iter().any(|d| {
                d["provenance"] == "provider_verified"
                    && d["status"] == "success"
                    && d["repository_id"] == merged["repository_id"]
                    && d["merge"]["merge_sha"] == merged["merge_sha"]
                    && d["merge"]["target_branch"] == merged["target_branch"]
                    && d["source_head_sha"] == merged["head_sha"]
            }) {
                return Err(PlatformError::Conflict(
                    "destination requires successful deployment of the exact merged revision",
                ));
            }
            if corrective
                && !merges.iter().any(|m| {
                    m["repository_id"] == merged["repository_id"]
                        && m["target_branch"] == "develop"
                        && m["head_sha"] == merged["head_sha"]
                        && m["provenance"] == "provider_verified"
                })
            {
                return Err(PlatformError::Conflict(
                    "integrate the corrective revision back into develop before delivery",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "task_delivery_evidence_tests.rs"]
mod tests;
