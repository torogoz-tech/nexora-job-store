use crate::{
    DatabaseConnections, bounded_json::BoundedJson, connection_secrets::ConnectionSecrets,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use nexora_core::NexoraError;
use serde_json::{Value, json};
use std::{io::Read, time::Duration};
pub(crate) struct Neo4jClient {
    agent: ureq::Agent,
    url: String,
    authorization: String,
}
impl Neo4jClient {
    pub fn new(c: &DatabaseConnections, s: &ConnectionSecrets) -> Self {
        let config = ureq::Agent::config_builder()
            .proxy(None)
            .max_redirects(0)
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_global(Some(Duration::from_secs(30)))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            url: format!("http://{}:{}/db/neo4j/tx/commit", c.host, c.neo4j_http_port),
            authorization: format!(
                "Basic {}",
                STANDARD.encode(format!("{}:{}", s.neo4j_user, s.neo4j_password))
            ),
        }
    }
    pub fn execute(
        &self,
        statement: &str,
        parameters: Value,
        timeout: Duration,
    ) -> Result<Vec<Value>, NexoraError> {
        let error = || NexoraError::Storage("Neo4j request failed".into());
        let bytes = BoundedJson::encode(
            &json!({"statements":[{"statement":statement,"parameters":parameters}]}),
            16_777_216,
        )?;
        let mut response = self
            .agent
            .post(&self.url)
            .header("Authorization", &self.authorization)
            .header("Content-Type", "application/json")
            .config()
            .timeout_global(Some(timeout.min(Duration::from_secs(30))))
            .build()
            .send(bytes.as_slice())
            .map_err(|failure| match failure {
                ureq::Error::Timeout(_) => NexoraError::Storage("Neo4j request timed out".into()),
                _ => error(),
            })?;
        if !response.status().is_success() {
            return Err(error());
        }
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(16_777_217)
            .read_to_end(&mut bytes)
            .map_err(|_| error())?;
        if bytes.len() > 16_777_216 {
            return Err(NexoraError::Limit("Neo4j response >16MiB".into()));
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| error())?;
        let failures = value
            .get("errors")
            .and_then(Value::as_array)
            .ok_or_else(error)?;
        if let Some(failure) = failures.first() {
            // Provider messages can contain source or query data. Only expose its code.
            let code = failure
                .get("code")
                .and_then(Value::as_str)
                .filter(|code| {
                    code.len() <= 128
                        && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.')
                })
                .unwrap_or("unknown");
            return Err(NexoraError::Storage(format!("Neo4j error: {code}")));
        }
        let rows = value
            .pointer("/results/0/data")
            .and_then(Value::as_array)
            .ok_or_else(error)?;
        rows.iter()
            .map(|row| row.get("row").cloned().ok_or_else(error))
            .collect()
    }
}
