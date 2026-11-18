use crate::{
    CacheRetention, DatabaseConnections, bounded_json::BoundedJson,
    connection_secrets::ConnectionSecrets,
};
use nexora_core::NexoraError;
use nexora_indexer::{CacheFragment, FileCacheKey};
use sha2::{Digest, Sha256};
use std::time::Duration;
pub struct DragonflyCache {
    client: redis::Client,
    connection: Option<redis::Connection>,
}
impl DragonflyCache {
    const LIMIT: usize = 67_108_864;
    pub(crate) fn connect(
        c: &DatabaseConnections,
        s: &ConnectionSecrets,
    ) -> Result<Self, NexoraError> {
        let client = redis::Client::open(redis::ConnectionInfo {
            addr: redis::ConnectionAddr::Tcp(c.host.clone(), c.redis_port),
            redis: redis::RedisConnectionInfo {
                password: Some(s.redis.clone()),
                ..Default::default()
            },
        })
        .map_err(|_| Self::error())?;
        Ok(Self {
            client,
            connection: None,
        })
    }
    pub(crate) fn connection(&mut self) -> Result<&mut redis::Connection, NexoraError> {
        if self.connection.is_none() {
            let connection = self
                .client
                .get_connection_with_timeout(Duration::from_secs(3))
                .map_err(|_| Self::error())?;
            connection
                .set_read_timeout(Some(Duration::from_secs(3)))
                .map_err(|_| Self::error())?;
            connection
                .set_write_timeout(Some(Duration::from_secs(3)))
                .map_err(|_| Self::error())?;
            self.connection = Some(connection);
        }
        self.connection.as_mut().ok_or_else(Self::error)
    }
    pub(crate) fn reset_connection(&mut self) {
        self.connection = None;
    }
    fn error() -> NexoraError {
        NexoraError::Storage("Dragonfly request failed".into())
    }
    pub fn key(key: &FileCacheKey) -> Result<String, NexoraError> {
        let bytes = BoundedJson::encode(key, 16_384)?;
        Ok(format!("nexora:analysis:v2:{:x}", Sha256::digest(bytes)))
    }
    pub fn ping(&mut self) -> Result<(), NexoraError> {
        let value: String = redis::cmd("PING")
            .query(self.connection()?)
            .map_err(|_| Self::error())?;
        if value != "PONG" {
            return Err(Self::error());
        }
        Ok(())
    }
    pub fn read(&mut self, key: &FileCacheKey) -> Result<Option<CacheFragment>, NexoraError> {
        let name = Self::key(key)?;
        let bytes: Vec<u8> = redis::cmd("GETRANGE")
            .arg(&name)
            .arg(0)
            .arg(Self::LIMIT)
            .query(self.connection()?)
            .map_err(|_| Self::error())?;
        if bytes.is_empty() {
            return Ok(None);
        }
        let fragment = if bytes.len() <= Self::LIMIT {
            serde_json::from_slice::<CacheFragment>(&bytes)
                .ok()
                .filter(|f| f.validate_for(key).is_ok())
        } else {
            None
        };
        if let Some(fragment) = fragment {
            let _: bool = redis::cmd("EXPIRE")
                .arg(&name)
                .arg(CacheRetention::seconds(
                    fragment.computation_us,
                    bytes.len(),
                ))
                .query(self.connection()?)
                .map_err(|_| Self::error())?;
            return Ok(Some(fragment));
        }
        // Preserve a concurrent repair; remove only the corrupt value observed or an oversized value.
        let _:i64=redis::cmd("EVAL").arg("local v=redis.call('GETRANGE',KEYS[1],0,tonumber(ARGV[2])); if string.len(v)>tonumber(ARGV[2]) or v==ARGV[1] then return redis.call('DEL',KEYS[1]) end return 0").arg(1).arg(name).arg(bytes).arg(Self::LIMIT).query(self.connection()?).map_err(|_|Self::error())?;
        Ok(None)
    }
    pub fn write(
        &mut self,
        key: &FileCacheKey,
        value: &CacheFragment,
    ) -> Result<bool, NexoraError> {
        value.validate_for(key)?;
        let bytes = match BoundedJson::encode(value, Self::LIMIT) {
            Ok(bytes) => bytes,
            Err(NexoraError::Limit(_)) => return Ok(false),
            Err(e) => return Err(e),
        };
        for _ in 0..2 {
            let result: redis::RedisResult<Option<String>> = redis::cmd("SET")
                .arg(Self::key(key)?)
                .arg(&bytes)
                .arg("NX")
                .arg("EX")
                .arg(CacheRetention::seconds(value.computation_us, bytes.len()))
                .query(self.connection()?);
            match result {
                Ok(Some(_)) => return Ok(true),
                Ok(None) => {
                    if let Some(existing) = self.read(key)? {
                        if existing.same_analysis(value)? {
                            return Ok(true);
                        }
                        return Err(NexoraError::Storage(
                            "conflicting analysis under immutable cache key".into(),
                        ));
                    }
                }
                Err(e) if e.code() == Some("OOM") => return Ok(false),
                Err(_) => return Err(Self::error()),
            }
        }
        Ok(false)
    }
}
