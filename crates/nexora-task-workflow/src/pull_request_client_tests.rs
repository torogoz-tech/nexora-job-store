use super::PullRequestClient;
use crate::GitNetworkPolicy;
use nexora_platform::{GitProvider, PlatformError};
use std::{
    io::{Read, Write},
    net::TcpListener,
};
#[test]
fn confines_credentials_and_never_follows_provider_redirects() {
    for (status, body) in [(200, "{\"items\":[]}"), (401, "{}"), (302, "{}")] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 8192];
            let n = socket.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..n]).to_ascii_lowercase();
            assert!(request.starts_with("get /repos/org/repo/pulls"));
            assert!(request.contains("authorization: bearer fixture-token"));
            write!(socket,"HTTP/1.1 {status} Test\r\nLocation: https://outside.invalid/steal\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        });
        let policy = GitNetworkPolicy {
            github_api_origin: origin,
            ..Default::default()
        };
        let client = PullRequestClient::new(&policy, GitProvider::Github);
        assert!(
            client
                .get("https://outside.invalid/api", "fixture-token")
                .is_err()
        );
        let result = client.get("/repos/org/repo/pulls", "fixture-token");
        match status {
            200 => assert!(result.is_ok()),
            401 => assert!(matches!(result, Err(PlatformError::Conflict(_)))),
            _ => assert!(matches!(result, Err(PlatformError::Unavailable))),
        }
        server.join().unwrap();
    }
}
