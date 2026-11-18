use crate::{
    TaskWorkflowStore, document_policy_store::DocumentPolicyStore,
    project_context_policy::ProjectContextPolicy,
};
use nexora_platform::*;
use postgres::Transaction;
use std::collections::BTreeSet;
pub(crate) struct TaskDocumentCondition;
impl TaskDocumentCondition {
    pub fn approved(tx: &mut Transaction<'_>, task: &WorkTask) -> Result<bool, PlatformError> {
        // The caller already holds the task row, so prerequisite mutations cannot race.
        if task.project_id.is_none() {
            return Ok(false);
        }
        let rows=tx.query("SELECT l.source_document_id,l.source_version_id,v.context_manifest_id FROM document_links l JOIN project_document_versions v ON v.id=l.source_version_id AND v.document_id=l.source_document_id AND v.project_id=l.project_id WHERE l.project_id=$1 AND l.target_task_id=$2 AND l.kind='required_for_transition' AND l.removed_at IS NULL ORDER BY l.source_document_id,l.source_version_id LIMIT 65",&[&task.project_scope()?,&task.id]).map_err(TaskWorkflowStore::database_error)?;
        if rows.is_empty() {
            return Ok(false);
        }
        if rows.len() > 64 {
            return Err(PlatformError::Validation(
                "task exceeds64 document prerequisites",
            ));
        }
        let mut manifests = BTreeSet::new();
        let mut documents = BTreeSet::new();
        for row in &rows {
            documents.insert(row.get::<_, String>(0));
            if let Some(id) = row.get::<_, Option<String>>(2) {
                manifests.insert(id);
            }
        }
        // Task -> source metadata -> document roots, matching link and document mutation order.
        for id in manifests {
            let manifest = ProjectContextPolicy::get(tx, task.project_scope()?, &id)?;
            ProjectContextPolicy::lock_sources(tx, &manifest)?;
        }
        for id in documents {
            tx.query_opt(
                "SELECT id FROM project_documents WHERE id=$1 AND project_id=$2 FOR SHARE",
                &[&id, &task.project_id],
            )
            .map_err(TaskWorkflowStore::database_error)?
            .ok_or(PlatformError::NotFound)?;
        }
        for row in rows {
            let document: String = row.get(0);
            let version: String = row.get(1);
            let root =
                DocumentPolicyStore::definition(tx, task.project_scope()?, &document, false)?;
            if root.archived || root.published_version_id.as_deref() != Some(version.as_str()) {
                return Ok(false);
            }
            let current =
                DocumentPolicyStore::version(tx, task.project_scope()?, &document, &version)?;
            if current.needs_review {
                return Ok(false);
            }
            let found=tx.query_opt("SELECT id FROM document_publications WHERE project_id=$1 AND document_id=$2 AND version_id=$3 AND checksum=$4 AND workflow_version_id IS NOT DISTINCT FROM $5 AND state_id=$6 AND state_revision=$7",&[&task.project_scope()?,&document,&version,&current.checksum,&current.workflow_version_id,&current.state_id,&current.state_revision]).map_err(TaskWorkflowStore::database_error)?.is_some();
            if !found {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
