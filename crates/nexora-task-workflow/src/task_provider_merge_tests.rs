use crate::{
    GitNetworkPolicy, pull_request_client::PullRequestClient,
    task_provider_merge::TaskProviderMerge,
};
use nexora_platform::{GitProvider, PlatformError};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
};
const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
fn fixture(
    provider: GitProvider,
    responses: Vec<(String, Option<Value>, u16, Value)>,
) -> (PullRequestClient, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for (path, expected, status, response) in responses {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0u8; 8192];
            let header_end = loop {
                let n = socket.read(&mut buf).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
                if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i + 4;
                }
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]).to_ascii_lowercase();
            assert!(headers.starts_with(&path.to_ascii_lowercase()), "{headers}");
            if let Some(expected) = expected {
                let len: usize = headers
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                while bytes.len() < header_end + len {
                    let n = socket.read(&mut buf).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buf[..n]);
                }
                assert_eq!(
                    serde_json::from_slice::<Value>(&bytes[header_end..header_end + len]).unwrap(),
                    expected
                );
            }
            let response = response.to_string();
            write!(socket,"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).unwrap();
        }
    });
    let policy = GitNetworkPolicy {
        github_api_origin: origin.clone(),
        gitlab_origin: origin,
        ..Default::default()
    };
    (PullRequestClient::new(&policy, provider), server)
}
#[test]
fn merges_use_exact_sha_and_do_not_request_automatic_or_background_merge() {
    for provider in [GitProvider::Github, GitProvider::Gitlab] {
        let (root, path, body) = if provider == GitProvider::Github {
            (
                "/repos/o/r",
                "PUT /repos/o/r/pulls/7/merge",
                json!({"sha":HEAD,"merge_method":"merge"}),
            )
        } else {
            (
                "/api/v4/projects/1",
                "PUT /api/v4/projects/1/merge_requests/7/merge",
                json!({"sha":HEAD,"auto_merge":false,"should_remove_source_branch":false}),
            )
        };
        let (client, server) = fixture(
            provider,
            vec![(path.into(), Some(body), 200, json!({"merged":true}))],
        );
        TaskProviderMerge::write(&client, provider, root, "fixture-token", 7, HEAD).unwrap();
        server.join().unwrap();
    }
}
#[test]
fn github_stale_review_does_not_satisfy_current_revision_approval() {
    let (client, server) = fixture(
        GitProvider::Github,
        vec![
            (
                "GET /repos/o/r/rules/branches/develop?per_page=100&page=1".into(),
                None,
                200,
                json!([]),
            ),
            (
                "GET /repos/o/r/branches/develop/protection".into(),
                None,
                200,
                json!({"required_pull_request_reviews":{"required_approving_review_count":1}}),
            ),
            (
                "GET /repos/o/r/pulls/7/reviews?per_page=100&page=1".into(),
                None,
                200,
                json!([{"id":1,"user":{"id":9},"state":"APPROVED","commit_id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]),
            ),
        ],
    );
    assert!(
        TaskProviderMerge::ready(
            &client,
            GitProvider::Github,
            "/repos/o/r",
            "fixture-token",
            &json!({"number":7,"draft":false,"mergeable":true,"mergeable_state":"clean"}),
            HEAD,
            "develop"
        )
        .is_err()
    );
    server.join().unwrap();
}
#[test]
fn github_merge_queue_is_never_bypassed() {
    let (client, server) = fixture(
        GitProvider::Github,
        vec![(
            "GET /repos/o/r/rules/branches/develop".into(),
            None,
            200,
            json!([{"type":"merge_queue"}]),
        )],
    );
    assert!(
        TaskProviderMerge::ready(
            &client,
            GitProvider::Github,
            "/repos/o/r",
            "fixture-token",
            &json!({"number":7,"draft":false,"mergeable":true,"mergeable_state":"clean"}),
            HEAD,
            "develop"
        )
        .is_err()
    );
    server.join().unwrap();
}
#[test]
fn gitlab_required_approvals_block_before_merge_or_pipeline_write() {
    let (client, server) = fixture(
        GitProvider::Gitlab,
        vec![(
            "GET /api/v4/projects/1/merge_requests/7/approvals".into(),
            None,
            200,
            json!({"approvals_left":1}),
        )],
    );
    assert!(TaskProviderMerge::ready(&client,GitProvider::Gitlab,"/api/v4/projects/1","fixture-token",&json!({"iid":7,"draft":false,"has_conflicts":false,"detailed_merge_status":"mergeable"}),HEAD,"develop").is_err());
    server.join().unwrap();
}
#[test]
fn unknown_merge_response_is_not_retried_by_transport() {
    let (client, server) = fixture(
        GitProvider::Github,
        vec![(
            "PUT /repos/o/r/pulls/7/merge".into(),
            Some(json!({"sha":HEAD,"merge_method":"merge"})),
            500,
            json!({}),
        )],
    );
    assert!(matches!(
        TaskProviderMerge::write(
            &client,
            GitProvider::Github,
            "/repos/o/r",
            "fixture-token",
            7,
            HEAD
        ),
        Err(PlatformError::Unavailable)
    ));
    server.join().unwrap();
}
#[test]
fn confirmed_merge_uses_provider_merge_revision_for_idempotent_reconciliation() {
    let result=TaskProviderMerge::confirmed(GitProvider::Github,&json!({"number":7,"merged":true,"merge_commit_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}),HEAD,"production").unwrap().unwrap();
    assert_eq!(
        result["merge_sha"],
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    );
    assert_eq!(result["head_sha"], HEAD);
    assert!(
        TaskProviderMerge::confirmed(
            GitProvider::Github,
            &json!({"number":7,"merged":true}),
            HEAD,
            "production"
        )
        .is_err()
    );
}
