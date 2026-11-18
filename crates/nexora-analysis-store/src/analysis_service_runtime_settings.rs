use nexora_platform::OpenAiConfiguration;
use zeroize::Zeroizing;

pub struct AnalysisServiceRuntimeSettings {
    pub settings: OpenAiConfiguration,
    pub secret: Option<Zeroizing<Vec<u8>>>,
}
