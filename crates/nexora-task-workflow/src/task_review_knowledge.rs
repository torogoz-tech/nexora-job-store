use crate::{
    TaskWorkflowStore, document_policy_store::DocumentPolicyStore,
    feature_governance_policy::FeatureGovernancePolicy,
};
use nexora_platform::*;
use postgres::Transaction;
use serde_json::{Value, json};
/// Authorized immutable versions and their actual publication evidence for a task review.
pub(crate) struct TaskReviewKnowledge;
impl TaskReviewKnowledge {
    fn requirement(
        tx: &mut Transaction<'_>,
        project: &str,
        id: &str,
        pinned: Option<&str>,
    ) -> Result<Value, PlatformError> {
        let document = DocumentPolicyStore::definition(tx, project, id, false)?;
        if document.archived || document.kind != WorkflowResourceKind::Requirement {
            return Err(PlatformError::Conflict(
                "linked review requirement is archived or has a different kind",
            ));
        }
        let version = DocumentPolicyStore::version(
            tx,
            project,
            id,
            pinned.unwrap_or(&document.latest_version_id),
        )?;
        let publication:Option<Value>=tx.query_opt("SELECT to_jsonb(p) FROM document_publications p WHERE project_id=$1 AND document_id=$2 AND version_id=$3 AND checksum=$4 ORDER BY created_at DESC LIMIT 1",&[&project,&id,&version.id,&version.checksum]).map_err(TaskWorkflowStore::database_error)?.map(|r|r.get(0));
        Ok(
            json!({"document_id":id,"document_revision":document.revision,"current_latest_version_id":document.latest_version_id,"is_current_published":document.published_version_id.as_deref()==Some(version.id.as_str()),"version":version,"publication":publication}),
        )
    }
    pub fn capture(
        store: &mut TaskWorkflowStore,
        ctx: &AuthContext,
        task: &WorkTask,
    ) -> Result<Value, PlatformError> {
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        let result = Self::capture_tx(&mut tx, ctx, task)?;
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok(result)
    }
    pub fn capture_tx(
        tx: &mut Transaction<'_>,
        ctx: &AuthContext,
        task: &WorkTask,
    ) -> Result<Value, PlatformError> {
        if task.primary_requirement_id.is_none() && task.primary_feature_id.is_none() {
            return Ok(Value::Null);
        }
        let project = task.project_id.as_deref().ok_or(PlatformError::Conflict(
            "primary review knowledge requires a project-linked task",
        ))?;
        DocumentPolicyStore::authorize(tx, ctx, project, Capability::DocumentsRead)?;
        let result = if let Some(id) = &task.primary_requirement_id {
            json!({"kind":"requirement","requirement":Self::requirement(tx,project,id,None)?})
        } else {
            FeatureGovernancePolicy::read(tx, ctx, project)?;
            let id = task
                .primary_feature_id
                .as_deref()
                .ok_or(PlatformError::Storage)?;
            let feature:Value=tx.query_opt("SELECT to_jsonb(f) FROM project_features f WHERE id=$1 AND project_id=$2 AND NOT archived",&[&id,&project]).map_err(TaskWorkflowStore::database_error)?.ok_or(PlatformError::NotFound)?.get(0);
            let links=tx.query("SELECT l.requirement_id,p.requirement_version_id,jsonb_build_object('impact',l.impact,'proposal_id',p.id,'proposal_revision',p.revision,'proposal_status',p.status,'decided_by',p.decided_by,'decided_at',p.decided_at) FROM requirement_features l JOIN feature_proposals p ON p.id=l.proposal_id AND p.project_id=l.project_id WHERE l.feature_id=$1 AND l.project_id=$2 AND p.status='approved' ORDER BY l.requirement_id",&[&id,&project]).map_err(TaskWorkflowStore::database_error)?;
            let mut requirements = Vec::new();
            for row in links {
                let requirement: String = row.get(0);
                let version: String = row.get(1);
                requirements.push(json!({"trace":row.get::<_,Value>(2),"requirement":Self::requirement(tx,project,&requirement,Some(&version))?}));
            }
            json!({"kind":"feature","feature":feature,"requirements":requirements})
        };
        Ok(result)
    }
    pub fn criteria(knowledge: &Value) -> Vec<Value> {
        let requirements: Vec<&Value> = if knowledge["kind"] == "requirement" {
            vec![&knowledge["requirement"]]
        } else {
            knowledge["requirements"]
                .as_array()
                .map(|r| r.iter().map(|r| &r["requirement"]).collect())
                .unwrap_or_default()
        };
        requirements.into_iter().enumerate().flat_map(|(r,requirement)|{
            requirement["version"]["content"]["body"]["acceptance_criteria"].as_array().into_iter().flatten().enumerate().map(move|(c,text)|json!({"id":format!("R{}C{}",r+1,c+1),"description":text,"source":{"document_id":requirement["document_id"],"version_id":requirement["version"]["id"],"checksum":requirement["version"]["checksum"],"publication":requirement["publication"]}}))
        }).collect()
    }
}
#[cfg(test)]
#[path = "task_review_knowledge_tests.rs"]
mod tests;
