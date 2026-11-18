use crate::{
    TaskWorkflowStore, git_author_identity::GitAuthorIdentity,
    pull_request_client::PullRequestClient,
};
use nexora_platform::*;
use serde_json::{Value, json};
/// Complete bounded provider commit graph and per-author patches; never trusts push identities.
pub(crate) struct TaskContributorCode;
impl TaskContributorCode {
    pub fn capture(
        client: &PullRequestClient,
        provider: GitProvider,
        root: &str,
        token: &str,
        base: &str,
        head: &str,
        review_files: &Value,
    ) -> Result<Value, PlatformError> {
        let github = provider == GitProvider::Github;
        let comparison = client.get(
            &if github {
                format!("{root}/compare/{base}...{head}?per_page=100&page=1")
            } else {
                format!("{root}/repository/compare?from={base}&to={head}&straight=true")
            },
            token,
        )?;
        if comparison["compare_timeout"] == true || comparison["overflow"] == true {
            return Err(PlatformError::Validation(
                "contributor comparison is incomplete",
            ));
        }
        let listed = comparison["commits"]
            .as_array()
            .ok_or(PlatformError::Unavailable)?;
        if listed.is_empty() || listed.len() > 32 {
            return Err(PlatformError::Validation(
                "contributor rating requires one complete bounded set of at most 32 commits",
            ));
        }
        if github
            && (comparison["total_commits"].as_u64() != Some(listed.len() as u64)
                || comparison["merge_base_commit"]["sha"] != base
                || comparison["base_commit"]["sha"] != base)
        {
            return Err(PlatformError::Validation(
                "contributor comparison is incomplete or requires rebasing",
            ));
        }
        if !github {
            let merge = client.get(
                &format!("{root}/repository/merge_base?refs%5B%5D={base}&refs%5B%5D={head}"),
                token,
            )?;
            if merge["id"] != base {
                return Err(PlatformError::Validation(
                    "contributor comparison requires a verified ancestor base",
                ));
            }
        }
        let actual = crate::task_pre_pr_review::TaskPrePrReview::files(
            client, provider, root, token, base, head,
        )?;
        if Self::canonical_files(&actual)? != Self::canonical_files(review_files)? {
            return Err(PlatformError::Conflict(
                "provider diff differs from the completed technical review",
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        for commit in listed {
            let sha = commit[if github { "sha" } else { "id" }]
                .as_str()
                .filter(|s| TaskWorkBranchReport::valid_sha(s))
                .ok_or(PlatformError::Unavailable)?;
            if sha == base || !ids.insert(sha.to_owned()) {
                return Err(PlatformError::Validation("invalid contributor commit set"));
            }
        }
        if !ids.contains(head) {
            return Err(PlatformError::Validation(
                "contributor comparison omitted the expected head",
            ));
        }
        let mut commits = Vec::new();
        let mut size = 0usize;
        for sha in &ids {
            let detail = client.get(
                &if github {
                    format!("{root}/commits/{sha}?per_page=100&page=1")
                } else {
                    format!("{root}/repository/commits/{sha}?stats=true")
                },
                token,
            )?;
            if detail[if github { "sha" } else { "id" }] != *sha {
                return Err(PlatformError::Conflict(
                    "provider returned another contributor commit",
                ));
            }
            let parents = if github {
                detail["parents"]
                    .as_array()
                    .ok_or(PlatformError::Unavailable)?
                    .iter()
                    .map(|p| {
                        p["sha"]
                            .as_str()
                            .map(str::to_owned)
                            .ok_or(PlatformError::Unavailable)
                    })
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                detail["parent_ids"]
                    .as_array()
                    .ok_or(PlatformError::Unavailable)?
                    .iter()
                    .map(|p| {
                        p.as_str()
                            .map(str::to_owned)
                            .ok_or(PlatformError::Unavailable)
                    })
                    .collect::<Result<Vec<_>, _>>()?
            };
            if parents.is_empty() || parents.iter().any(|p| p != base && !ids.contains(p)) {
                return Err(PlatformError::Validation(
                    "provider comparison omitted an ancestor commit",
                ));
            }
            let email = if github {
                detail["commit"]["author"]["email"].as_str()
            } else {
                detail["author_email"].as_str()
            }
            .and_then(GitAuthorIdentity::email);
            // Merge author is not credited with changes authored by merged contributors.
            let files = if parents.len() > 1 {
                json!([])
            } else {
                let diff = if github {
                    detail["files"].clone()
                } else {
                    client.get(
                        &format!("{root}/repository/commits/{sha}/diff?per_page=100&page=1"),
                        token,
                    )?
                };
                let entries =
                    diff.as_array()
                        .filter(|v| v.len() < 100)
                        .ok_or(PlatformError::Validation(
                            "contributor commit diff reaches provider pagination limit",
                        ))?;
                let mut files = Vec::new();
                let mut additions = 0u64;
                let mut deletions = 0u64;
                for file in entries {
                    if file["collapsed"] == true || file["too_large"] == true {
                        return Err(PlatformError::Validation(
                            "contributor commit diff is truncated",
                        ));
                    }
                    let patch = file[if github { "patch" } else { "diff" }]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .ok_or(PlatformError::Validation(
                            "binary or omitted contributor diff cannot be rated",
                        ))?;
                    let path = file[if github { "filename" } else { "new_path" }]
                        .as_str()
                        .filter(|s| !s.is_empty() && s.len() <= 4096)
                        .ok_or(PlatformError::Unavailable)?;
                    let plus = patch.lines().filter(|l| l.starts_with('+')).count() as u64;
                    let minus = patch.lines().filter(|l| l.starts_with('-')).count() as u64;
                    if github
                        && (file["additions"].as_u64() != Some(plus)
                            || file["deletions"].as_u64() != Some(minus))
                    {
                        return Err(PlatformError::Validation(
                            "contributor commit patch is incomplete",
                        ));
                    }
                    additions += plus;
                    deletions += minus;
                    size += patch.len();
                    files.push(json!({"path":path,"patch":patch}));
                }
                if detail["stats"]["additions"].as_u64() != Some(additions)
                    || detail["stats"]["deletions"].as_u64() != Some(deletions)
                {
                    return Err(PlatformError::Validation(
                        "contributor commit statistics do not match complete patches",
                    ));
                }
                json!(files)
            };
            if size > 192 * 1024 {
                return Err(PlatformError::Validation(
                    "contributor commit evidence exceeds rating budget",
                ));
            }
            commits.push(json!({"sha":sha,"author_email":email,"parents":parents,"files":files}));
        }
        let mut reachable = std::collections::BTreeSet::new();
        let mut pending = vec![head.to_owned()];
        while let Some(sha) = pending.pop() {
            if sha == base || !reachable.insert(sha.clone()) {
                continue;
            }
            let row = commits
                .iter()
                .find(|c| c["sha"] == sha)
                .ok_or(PlatformError::Unavailable)?;
            for parent in row["parents"]
                .as_array()
                .ok_or(PlatformError::Unavailable)?
            {
                pending.push(
                    parent
                        .as_str()
                        .ok_or(PlatformError::Unavailable)?
                        .to_owned(),
                );
            }
        }
        if reachable != ids {
            return Err(PlatformError::Validation(
                "comparison includes commits outside the expected head ancestry",
            ));
        }
        Ok(json!(commits))
    }
    fn canonical_files(v: &Value) -> Result<Vec<(String, String)>, PlatformError> {
        let mut result = v
            .as_array()
            .ok_or(PlatformError::Unavailable)?
            .iter()
            .map(|f| {
                Ok((
                    f["path"]
                        .as_str()
                        .ok_or(PlatformError::Unavailable)?
                        .to_owned(),
                    f["patch"]
                        .as_str()
                        .ok_or(PlatformError::Unavailable)?
                        .to_owned(),
                ))
            })
            .collect::<Result<Vec<_>, PlatformError>>()?;
        result.sort();
        Ok(result)
    }
    pub fn contributors(
        store: &mut TaskWorkflowStore,
        project: &str,
        repository: &str,
        provider: &str,
        host: &str,
        remote: &str,
        commits: &Value,
    ) -> Result<Vec<Value>, PlatformError> {
        let mut tx = store
            .client()
            .transaction()
            .map_err(TaskWorkflowStore::database_error)?;
        crate::git_author_policy::GitAuthorPolicy::lock(&mut tx)?;
        let mut grouped: std::collections::BTreeMap<String, (Vec<Value>, Vec<Value>)> =
            std::collections::BTreeMap::new();
        for commit in commits.as_array().ok_or(PlatformError::Storage)? {
            if commit["files"].as_array().is_none_or(Vec::is_empty) {
                continue;
            }
            let Some(email) = commit["author_email"].as_str() else {
                continue;
            };
            let sha = commit["sha"].as_str().ok_or(PlatformError::Storage)?;
            let rows=tx.query("SELECT c.actor_id,c.id,c.alias_id,a.id,a.revision,r.revision FROM git_commit_authorship c JOIN git_author_aliases a ON a.provider=c.provider AND a.host=c.host AND a.email=c.author_email AND a.actor_id=c.actor_id AND a.revoked_at IS NULL AND a.project_id IS NULL JOIN actors member ON member.id=c.actor_id AND member.active AND member.kind='human' JOIN actor_roles r ON r.actor_id=member.id AND r.role IN ('developer','technical_lead') WHERE c.project_id=$1 AND c.repository_id=$2 AND c.provider=$3 AND c.host=$4 AND c.remote_repository_id=$5 AND c.commit_sha=$6 AND c.author_email=$7 FOR SHARE OF c,a,member,r",&[&project,&repository,&provider,&host,&remote,&sha,&email]).map_err(TaskWorkflowStore::database_error)?;
            if rows.len() != 1 {
                continue;
            }
            let row = &rows[0];
            let actor: String = row.get(0);
            let entry = grouped.entry(actor).or_default();
            entry.0.push(commit.clone());
            entry.1.push(json!({"commit_sha":sha,"observation_id":row.get::<_,String>(1),"recorded_alias_id":row.get::<_,String>(2),"active_alias_id":row.get::<_,String>(3),"alias_revision":row.get::<_,i64>(4),"role_revision":row.get::<_,i64>(5)}));
        }
        if grouped.len() > 16 {
            return Err(PlatformError::Validation(
                "too many contributors for one bounded private rating action",
            ));
        }
        let result=grouped.into_iter().map(|(id,(commits,attribution))|json!({"actor_id":id,"commits":commits,"attribution":attribution})).collect();
        tx.commit().map_err(TaskWorkflowStore::database_error)?;
        Ok(result)
    }
}
#[cfg(test)]
#[path = "task_contributor_code_tests.rs"]
mod tests;
