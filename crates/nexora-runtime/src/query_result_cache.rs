use crate::{
    CacheRetention, DragonflyCache, KnowledgeQuery, QuerySnapshot, bounded_json::BoundedJson,
    query_cache_record::QueryCacheRecord,
};
use nexora_core::NexoraError;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub struct QueryResultCache;
impl QueryResultCache {
    const LIMIT: usize = 1_048_576;
    fn error() -> NexoraError {
        NexoraError::Storage("Dragonfly query cache unavailable".into())
    }
    pub fn key(head: &QuerySnapshot, query: &KnowledgeQuery) -> Result<String, NexoraError> {
        let bytes = BoundedJson::encode(
            &(
                QuerySnapshot::VERSION,
                &head.tenant_id,
                &head.repository_id,
                &head.id,
                &head.payload_hash,
                query,
            ),
            65_536,
        )?;
        Ok(format!("nexora:query:v1:{:x}", Sha256::digest(bytes)))
    }
    pub(crate) fn read(
        cache: &mut DragonflyCache,
        key: &str,
    ) -> Result<Option<Value>, NexoraError> {
        let bytes: Vec<u8> = redis::cmd("GETRANGE")
            .arg(key)
            .arg(0)
            .arg(Self::LIMIT)
            .query(cache.connection()?)
            .map_err(|_| Self::error())?;
        if bytes.is_empty() {
            return Ok(None);
        }
        let record = if bytes.len() <= Self::LIMIT {
            serde_json::from_slice::<QueryCacheRecord>(&bytes)
                .ok()
                .filter(|r| r.valid(key))
        } else {
            None
        };
        if let Some(record) = record {
            let _: bool = redis::cmd("EXPIRE")
                .arg(key)
                .arg(CacheRetention::seconds(record.computation_us, bytes.len()))
                .query(cache.connection()?)
                .map_err(|_| Self::error())?;
            return Ok(Some(record.data));
        }
        let _:i64=redis::cmd("EVAL").arg("if redis.call('GETRANGE',KEYS[1],0,tonumber(ARGV[2]))==ARGV[1] then return redis.call('DEL',KEYS[1]) end return 0").arg(1).arg(key).arg(bytes).arg(Self::LIMIT).query(cache.connection()?).map_err(|_|Self::error())?;
        Ok(None)
    }
    pub(crate) fn write(
        cache: &mut DragonflyCache,
        key: &str,
        data: &Value,
        cost: u64,
    ) -> Result<bool, NexoraError> {
        let record = match QueryCacheRecord::new(key.into(), data.clone(), cost) {
            Ok(v) => v,
            Err(NexoraError::Limit(_)) => return Ok(false),
            Err(e) => return Err(e),
        };
        let bytes = match BoundedJson::encode(&record, Self::LIMIT) {
            Ok(v) => v,
            Err(NexoraError::Limit(_)) => return Ok(false),
            Err(e) => return Err(e),
        };
        for _ in 0..2 {
            let result: redis::RedisResult<Option<String>> = redis::cmd("SET")
                .arg(key)
                .arg(&bytes)
                .arg("NX")
                .arg("EX")
                .arg(CacheRetention::seconds(cost, bytes.len()))
                .query(cache.connection()?);
            match result {
                Ok(Some(_)) => return Ok(true),
                Ok(None) => {
                    if let Some(existing) = Self::read(cache, key)? {
                        if existing == *data {
                            return Ok(true);
                        }
                        return Err(NexoraError::Storage(
                            "conflicting immutable query cache value".into(),
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
