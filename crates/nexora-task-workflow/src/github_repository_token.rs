use zeroize::Zeroizing;

pub(crate) struct GithubRepositoryToken {
    pub access_token: Zeroizing<String>,
    pub full_name: String,
}
