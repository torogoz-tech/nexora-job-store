use nexora_platform::GitProvider;
/// Separates corporate identity credentials from Git repository credentials.
#[derive(Clone, Copy)]
pub enum SecretAuthority {
    Git(GitProvider),
    OpenAi,
}
impl SecretAuthority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Git(provider) => provider.as_str(),
            Self::OpenAi => "openai",
        }
    }
}
impl From<GitProvider> for SecretAuthority {
    fn from(provider: GitProvider) -> Self {
        Self::Git(provider)
    }
}
