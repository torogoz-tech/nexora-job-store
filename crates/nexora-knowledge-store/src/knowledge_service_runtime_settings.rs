use serde_json::Value;
use zeroize::Zeroizing;

pub struct KnowledgeServiceRuntimeSettings {
    pub settings: Value,
    pub secret: Option<Zeroizing<Vec<u8>>>,
}
