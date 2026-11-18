use super::TaskProviderOperations;
use crate::{GitNetworkPolicy, pull_request_client::PullRequestClient};
use nexora_platform::{GitProvider, PlatformError};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MERGED: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
fn server(
    provider: GitProvider,
    responses: Vec<(&str, Value)>,
) -> (PullRequestClient, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let responses: Vec<_> = responses
        .into_iter()
        .map(|(path, body)| (path.to_owned(), body))
        .collect();
    let handle = std::thread::spawn(move || {
        for (path, body) in responses {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut raw = Vec::new();
            let mut buf = [0u8; 8192];
            let header_end = loop {
                let n = socket.read(&mut buf).unwrap();
                assert!(n > 0);
                raw.extend_from_slice(&buf[..n]);
                if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i + 4;
                }
            };
            let header = String::from_utf8_lossy(&raw[..header_end]).to_ascii_lowercase();
            assert!(header.contains("authorization: bearer fixture-token"));
            assert!(
                header.starts_with(&path.to_ascii_lowercase()),
                "unexpected request: {header}"
            );
            if header.starts_with("post ") {
                let len: usize = header
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                while raw.len() < header_end + len {
                    let n = socket.read(&mut buf).unwrap();
                    assert!(n > 0);
                    raw.extend_from_slice(&buf[..n]);
                }
                let sent: Value =
                    serde_json::from_slice(&raw[header_end..header_end + len]).unwrap();
                if provider == GitProvider::Github {
                    assert_eq!(sent, json!({"ref":"refs/heads/work/test","sha":SHA}));
                } else {
                    assert_eq!(sent, json!({"branch":"work/test","ref":SHA}));
                }
            }
            let body = body.to_string();
            let status = if header.starts_with("post ") {
                201
            } else {
                200
            };
            write!(socket, "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    let policy = GitNetworkPolicy {
        github_api_origin: origin.clone(),
        gitlab_origin: origin,
        ..Default::default()
    };
    (PullRequestClient::new(&policy, provider), handle)
}
#[test]
fn creates_both_provider_branches_from_exact_sha_never_moving_ref() {
    for (provider, path) in [
        (GitProvider::Github, "POST /repos/o/r/git/refs"),
        (
            GitProvider::Gitlab,
            "POST /api/v4/projects/1/repository/branches",
        ),
    ] {
        let root = if provider == GitProvider::Github {
            "/repos/o/r"
        } else {
            "/api/v4/projects/1"
        };
        let (client, handle) = server(provider, vec![(path, json!({"ok":true}))]);
        TaskProviderOperations::create_branch(
            &client,
            provider,
            root,
            "fixture-token",
            "work/test",
            SHA,
        )
        .unwrap();
        handle.join().unwrap();
        assert!(matches!(
            TaskProviderOperations::create_branch(
                &client,
                provider,
                root,
                "fixture-token",
                "work/test",
                "develop"
            ),
            Err(PlatformError::Validation(_))
        ));
    }
}
#[test]
fn github_requires_actual_successful_checks_at_exact_revision() {
    for checks in [
        json!([]),
        json!([{"name":"build","head_sha":SHA,"status":"in_progress","conclusion":null}]),
        json!([{"name":"build","head_sha":MERGED,"status":"completed","conclusion":"success"}]),
    ] {
        let no_checks = checks.as_array().unwrap().is_empty();
        let check_body =
            json!({"total_count":checks.as_array().unwrap().len(),"check_runs":checks});
        let mut responses = vec![(
            format!("GET /repos/o/r/commits/{SHA}/check-runs?filter=latest&per_page=100"),
            check_body,
        )];
        if no_checks {
            responses.push((
                format!("GET /repos/o/r/commits/{SHA}/status?per_page=100"),
                json!({"sha":SHA,"total_count":0,"statuses":[]}),
            ));
        }
        let (client, handle) = server(
            GitProvider::Github,
            responses
                .iter()
                .map(|(p, b)| (p.as_str(), b.clone()))
                .collect(),
        );
        assert!(
            TaskProviderOperations::pipeline(
                &client,
                GitProvider::Github,
                "/repos/o/r",
                "fixture-token",
                SHA,
                None
            )
            .is_err()
        );
        handle.join().unwrap();
    }
}
#[test]
fn github_checks_and_legacy_statuses_both_have_to_pass() {
    for state in ["failure", "success"] {
        let (client, handle) = server(
            GitProvider::Github,
            vec![
                (
                    &format!("GET /repos/o/r/commits/{SHA}/check-runs"),
                    json!({"total_count":1,"check_runs":[{"name":"build","head_sha":SHA,"status":"completed","conclusion":"success"}]}),
                ),
                (
                    &format!("GET /repos/o/r/commits/{SHA}/status"),
                    json!({"sha":SHA,"total_count":1,"statuses":[{"context":"security","state":state}]}),
                ),
            ],
        );
        assert_eq!(
            TaskProviderOperations::pipeline(
                &client,
                GitProvider::Github,
                "/repos/o/r",
                "fixture-token",
                SHA,
                None
            )
            .is_ok(),
            state == "success"
        );
        handle.join().unwrap();
    }
}
#[test]
fn latest_gitlab_pipeline_failure_cannot_be_masked_by_old_success() {
    let (client, handle) = server(
        GitProvider::Gitlab,
        vec![(
            &format!("GET /api/v4/projects/1/pipelines?sha={SHA}"),
            json!([{"id":2,"sha":SHA,"status":"failed"},{"id":1,"sha":SHA,"status":"success"}]),
        )],
    );
    assert!(
        TaskProviderOperations::pipeline(
            &client,
            GitProvider::Gitlab,
            "/api/v4/projects/1",
            "fixture-token",
            SHA,
            None
        )
        .is_err()
    );
    handle.join().unwrap();
}
#[test]
fn merged_request_requires_original_pinned_head_and_returns_real_merge_sha() {
    for provider in [GitProvider::Github, GitProvider::Gitlab] {
        let root = if provider == GitProvider::Github {
            "/repos/o/r"
        } else {
            "/api/v4/projects/1"
        };
        let path = if provider == GitProvider::Github {
            format!("GET {root}/commits/{SHA}/pulls")
        } else {
            format!("GET {root}/merge_requests?state=merged")
        };
        let pr = if provider == GitProvider::Github {
            json!({"number":7,"head":{"ref":"work/test","sha":SHA},"merged_at":"2026-11-11","merge_commit_sha":MERGED,"base":{"ref":"develop"}})
        } else {
            json!({"iid":7,"source_branch":"work/test","sha":SHA,"state":"merged","merge_commit_sha":MERGED,"target_branch":"develop"})
        };
        let (client, handle) = server(provider, vec![(&path, json!([pr]))]);
        let result = TaskProviderOperations::merged(
            &client,
            provider,
            root,
            "fixture-token",
            "work/test",
            SHA,
        )
        .unwrap();
        assert_eq!(result["merge_sha"], MERGED);
        assert_eq!(result["target_branch"], "develop");
        handle.join().unwrap();
    }
}
#[test]
fn deployment_success_on_other_revision_or_environment_does_not_pass() {
    let (client, handle) = server(
        GitProvider::Gitlab,
        vec![(
            "GET /api/v4/projects/1/deployments?environment=production",
            json!([
                {"id":3,"sha":MERGED,"environment":{"name":"production"},"status":"success"},
                {"id":2,"sha":SHA,"environment":{"name":"staging"},"status":"success"}
            ]),
        )],
    );
    assert!(
        TaskProviderOperations::deployment(
            &client,
            GitProvider::Gitlab,
            "/api/v4/projects/1",
            "fixture-token",
            SHA,
            "production"
        )
        .is_err()
    );
    handle.join().unwrap();
}
#[test]
fn latest_deployment_failure_cannot_be_masked_by_old_success() {
    let (client, handle) = server(
        GitProvider::Github,
        vec![
            (
                "GET /repos/o/r/deployments?environment=production",
                json!([
                    {"id":1,"sha":SHA,"environment":"production"}, {"id":2,"sha":SHA,"environment":"production"}
                ]),
            ),
            (
                "GET /repos/o/r/deployments/2/statuses?per_page=1",
                json!([{"state":"failure"}]),
            ),
        ],
    );
    assert!(
        TaskProviderOperations::deployment(
            &client,
            GitProvider::Github,
            "/repos/o/r",
            "fixture-token",
            SHA,
            "production"
        )
        .is_err()
    );
    handle.join().unwrap();
}
#[test]
fn provider_checks_are_fully_paginated_before_success() {
    let checks:Vec<_>=(0..100).map(|i|json!({"id":i,"name":format!("check-{i}"),"head_sha":SHA,"status":"completed","conclusion":"success"})).collect();
    let (client, handle) = server(
        GitProvider::Github,
        vec![
            (
                &format!(
                    "GET /repos/o/r/commits/{SHA}/check-runs?filter=latest&per_page=100&page=1"
                ),
                json!({"total_count":101,"check_runs":checks}),
            ),
            (
                &format!(
                    "GET /repos/o/r/commits/{SHA}/check-runs?filter=latest&per_page=100&page=2"
                ),
                json!({"total_count":101,"check_runs":[{"id":100,"name":"late-failure","head_sha":SHA,"status":"completed","conclusion":"failure"}]}),
            ),
        ],
    );
    assert!(
        TaskProviderOperations::pipeline(
            &client,
            GitProvider::Github,
            "/repos/o/r",
            "fixture-token",
            SHA,
            None
        )
        .is_err()
    );
    handle.join().unwrap();
}
#[test]
fn explicit_merge_target_selects_stage_independently_of_develop_backintegration() {
    let (client, handle) = server(
        GitProvider::Gitlab,
        vec![(
            "GET /api/v4/projects/1/merge_requests?state=merged",
            json!([
                {"iid":1,"source_branch":"work/test","sha":SHA,"state":"merged","merge_commit_sha":MERGED,"target_branch":"develop"},
                {"iid":2,"source_branch":"work/test","sha":SHA,"state":"merged","merge_commit_sha":SHA,"target_branch":"qa"}
            ]),
        )],
    );
    let result = TaskProviderOperations::merged_to(
        &client,
        GitProvider::Gitlab,
        "/api/v4/projects/1",
        "fixture-token",
        "work/test",
        SHA,
        Some("qa"),
    )
    .unwrap();
    assert_eq!(result["target_branch"], "qa");
    assert_eq!(result["merge_sha"], SHA);
    handle.join().unwrap();
}
#[test]
fn missing_or_truncated_pre_pr_diff_is_never_sent_as_complete_evidence() {
    for file in [
        json!({"filename":"source.rs","additions":2,"deletions":1,"patch":"@@ -1 +1 @@\n-old\n+new"}),
        json!({"filename":"image.png","additions":0,"deletions":0}),
    ] {
        let (client, handle) = server(
            GitProvider::Github,
            vec![(
                &format!("GET /repos/o/r/compare/{SHA}...{MERGED}"),
                json!({"files":[file]}),
            )],
        );
        assert!(
            crate::task_pre_pr_review::TaskPrePrReview::files(
                &client,
                GitProvider::Github,
                "/repos/o/r",
                "fixture-token",
                SHA,
                MERGED
            )
            .is_err()
        );
        handle.join().unwrap();
    }
}

#[test]
fn older_success_does_not_mask_environment_rollback_to_another_revision() {
    let (client, handle) = server(
        GitProvider::Gitlab,
        vec![(
            "GET /api/v4/projects/1/deployments?environment=production",
            json!([
                {"id":1,"sha":SHA,"environment":{"name":"production"},"status":"success"},
                {"id":2,"sha":MERGED,"environment":{"name":"production"},"status":"success"}
            ]),
        )],
    );
    assert!(
        TaskProviderOperations::deployment(
            &client,
            GitProvider::Gitlab,
            "/api/v4/projects/1",
            "fixture-token",
            SHA,
            "production"
        )
        .is_err()
    );
    handle.join().unwrap();
}
