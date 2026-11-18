use super::TaskContributorCode;
use crate::{GitNetworkPolicy, pull_request_client::PullRequestClient};
use nexora_platform::GitProvider;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
};
const BASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PATCH: &str = "@@ -1 +1 @@\n-old();\n+new();";
fn server(responses: Vec<Value>) -> (PullRequestClient, std::thread::JoinHandle<()>) {
    server_for(GitProvider::Github, responses)
}
fn server_for(
    provider: GitProvider,
    responses: Vec<Value>,
) -> (PullRequestClient, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let h = std::thread::spawn(move || {
        for body in responses {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut b = [0; 8192];
            let n = socket.read(&mut b).unwrap();
            assert!(String::from_utf8_lossy(&b[..n]).starts_with("GET "));
            let body = body.to_string();
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    (
        PullRequestClient::new(
            &GitNetworkPolicy {
                github_api_origin: origin.clone(),
                gitlab_origin: origin,
                ..Default::default()
            },
            provider,
        ),
        h,
    )
}
fn comparison() -> Value {
    json!({"total_commits":1,"commits":[{"sha":HEAD}],"merge_base_commit":{"sha":BASE},"base_commit":{"sha":BASE},"files":[{"filename":"a.rs","patch":PATCH,"additions":1,"deletions":1}]})
}
fn detail() -> Value {
    json!({"sha":HEAD,"parents":[{"sha":BASE}],"commit":{"author":{"email":" AUTHOR@EXAMPLE.COM "}},"stats":{"additions":1,"deletions":1},"files":[{"filename":"a.rs","patch":PATCH,"additions":1,"deletions":1}]})
}
#[test]
fn exact_provider_commits_include_author_owned_patch_without_pusher() {
    let (c, h) = server(vec![comparison(), comparison(), detail()]);
    let result = TaskContributorCode::capture(
        &c,
        GitProvider::Github,
        "/repos/o/r",
        "fixture",
        BASE,
        HEAD,
        &json!([{"path":"a.rs","patch":PATCH}]),
    )
    .unwrap();
    h.join().unwrap();
    assert_eq!(result[0]["author_email"], "author@example.com");
    assert_eq!(result[0]["files"][0]["patch"], PATCH);
    assert!(result[0].get("pusher").is_none());
}
#[test]
fn missing_commits_and_incomplete_per_commit_patches_fail_closed() {
    let mut incomplete = comparison();
    incomplete["total_commits"] = json!(2);
    let (c, h) = server(vec![incomplete]);
    assert!(
        TaskContributorCode::capture(
            &c,
            GitProvider::Github,
            "/repos/o/r",
            "fixture",
            BASE,
            HEAD,
            &json!([])
        )
        .is_err()
    );
    h.join().unwrap();
    let mut missing = detail();
    missing["stats"]["additions"] = json!(2);
    let (c, h) = server(vec![comparison(), comparison(), missing]);
    assert!(
        TaskContributorCode::capture(
            &c,
            GitProvider::Github,
            "/repos/o/r",
            "fixture",
            BASE,
            HEAD,
            &json!([{"path":"a.rs","patch":PATCH}])
        )
        .is_err()
    );
    h.join().unwrap();
}
#[test]
fn changed_review_snapshot_is_rejected_before_any_personal_model_call() {
    let (c, h) = server(vec![comparison(), comparison()]);
    assert!(
        TaskContributorCode::capture(
            &c,
            GitProvider::Github,
            "/repos/o/r",
            "fixture",
            BASE,
            HEAD,
            &json!([{"path":"a.rs","patch":"different"}])
        )
        .is_err()
    );
    h.join().unwrap();
}

#[test]
fn gitlab_author_requires_exact_complete_commit_diff() {
    let comparison = json!({"commits":[{"id":HEAD}],"diffs":[{"new_path":"a.rs","diff":PATCH}]});
    let (client, thread) = server_for(
        GitProvider::Gitlab,
        vec![
            comparison.clone(),
            json!({"id":BASE}),
            comparison,
            json!({"id":HEAD,"parent_ids":[BASE],"author_email":"dev@corporate","stats":{"additions":1,"deletions":1}}),
            json!([{"new_path":"a.rs","diff":PATCH}]),
        ],
    );
    let result = TaskContributorCode::capture(
        &client,
        GitProvider::Gitlab,
        "/api/v4/projects/123",
        "fixture",
        BASE,
        HEAD,
        &json!([{"path":"a.rs","patch":PATCH}]),
    )
    .unwrap();
    thread.join().unwrap();
    assert_eq!(result[0]["author_email"], "dev@corporate");
}
