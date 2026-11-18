pub(crate) struct GitAuthorIdentity;
impl GitAuthorIdentity {
    pub fn email(value: &str) -> Option<String> {
        let v = value.trim().to_lowercase();
        let parts: Vec<_> = v.split('@').collect();
        if v.len() > 320
            || parts.len() != 2
            || parts[0].is_empty()
            || parts[1].is_empty()
            || v.chars().any(|c| {
                c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | '[' | ']')
            })
        {
            None
        } else {
            Some(v)
        }
    }
}
