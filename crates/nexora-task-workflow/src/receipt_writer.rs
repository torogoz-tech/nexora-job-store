use crate::{TaskWorkflowStore, task_workflow_request_context::TaskWorkflowRequestContext};
use nexora_platform::PlatformError;
use serde_json::Value;
use subtle::ConstantTimeEq;
pub(crate) struct ReceiptWriter;
impl ReceiptWriter {
    pub fn read(
        client: &mut impl postgres::GenericClient,
        namespace: &str,
        request: Option<&TaskWorkflowRequestContext>,
        actor: &str,
        operation: &str,
    ) -> Result<Option<Value>, PlatformError> {
        let Some(request) = request else {
            return Ok(None);
        };
        if request.actor_id != actor || request.operation != operation {
            return Err(PlatformError::Conflict(
                "mutation request context does not match operation",
            ));
        }
        let lock = serde_json::to_string(&(namespace, actor, operation, &request.request_id))
            .map_err(|_| PlatformError::Storage)?;
        client
            .query_one(
                "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
                &[&lock],
            )
            .map_err(TaskWorkflowStore::database_error)?;
        let row=client.query_opt("SELECT request_hash,response FROM request_receipts WHERE actor_id=$1 AND operation=$2 AND request_id=$3",&[&actor,&operation,&request.request_id]).map_err(TaskWorkflowStore::database_error)?;
        row.map(|row| {
            let old: String = row.get(0);
            if !bool::from(old.as_bytes().ct_eq(request.fingerprint.as_bytes())) {
                return Err(PlatformError::Conflict(
                    "request ID was already used with different input",
                ));
            }
            let bytes: Vec<u8> = row.get(1);
            if bytes.len() > 1_048_576 {
                return Err(PlatformError::Storage);
            }
            serde_json::from_slice(&bytes).map_err(|_| PlatformError::Storage)
        })
        .transpose()
    }
    pub fn save(
        client: &mut impl postgres::GenericClient,
        request: Option<&TaskWorkflowRequestContext>,
        response: &Value,
    ) -> Result<(), PlatformError> {
        let Some(request) = request else {
            return Ok(());
        };
        let bytes = serde_json::to_vec(response).map_err(|_| PlatformError::Storage)?;
        if bytes.len() > 1_048_576 {
            return Err(PlatformError::Validation("receipt response exceeds limit"));
        }
        client.execute("INSERT INTO request_receipts(actor_id,operation,request_id,request_hash,response) VALUES($1,$2,$3,$4,$5)",&[&request.actor_id,&request.operation,&request.request_id,&request.fingerprint,&bytes]).map_err(TaskWorkflowStore::database_error)?;
        Ok(())
    }
}
