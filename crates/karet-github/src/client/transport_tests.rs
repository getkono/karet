//! Error mapping, pagination arithmetic, and one real HTTP round trip over a
//! loopback listener.

use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::thread::JoinHandle;

use bytes::Bytes;
use reqwest::StatusCode;

use super::*;

fn repository() -> RepositoryIdentity {
    RepositoryIdentity {
        owner: "getkono".to_string(),
        repo: "karet".to_string(),
    }
}

fn request_id_headers(id: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("x-github-request-id", HeaderValue::from_static(id));
    headers
}

#[test]
fn next_page_advances_only_while_items_remain() {
    // PER_PAGE is 50: page 1 covers items 1..=50.
    assert_eq!(next_page(1, 0), None);
    assert_eq!(
        next_page(1, 50),
        None,
        "an exactly full page has no successor"
    );
    assert_eq!(next_page(1, 51), Some(2));
    assert_eq!(next_page(2, 100), None);
    assert_eq!(next_page(2, 101), Some(3));
    // The product is computed in u64, so a large page never wraps into "more".
    assert_eq!(
        next_page(u32::MAX, u64::MAX),
        None,
        "no page after u32::MAX"
    );
    assert_eq!(
        next_page(u32::MAX - 1, u64::MAX),
        Some(u32::MAX),
        "the last representable page is still reachable"
    );
    assert_eq!(
        next_page(u32::MAX - 1, u64::from(u32::MAX - 1) * u64::from(PER_PAGE)),
        None
    );
}

#[test]
fn api_error_message_prefers_the_documented_body() {
    let body = br#"{"message":"Validation Failed","errors":[{"code":"missing"}],"documentation_url":"https://docs.github.com/x"}"#;
    assert_eq!(
        api_error_message(body),
        "Validation Failed Details: [{\"code\":\"missing\"}] Documentation: https://docs.github.com/x"
    );
    assert_eq!(
        api_error_message(br#"{"message":"Not Found"}"#),
        "Not Found"
    );
    // A body that is not GitHub's error shape falls back to a compact excerpt.
    assert_eq!(
        api_error_message(b"<html>\n  Bad   gateway\n</html>"),
        "<html> Bad gateway </html>"
    );
}

#[test]
fn response_excerpt_is_compacted_and_capped() {
    assert_eq!(compact_response_excerpt(b"  a \n\t b  "), "a b");
    assert_eq!(compact_response_excerpt(b""), "");
    let exact = vec![b'x'; 1_024];
    assert_eq!(compact_response_excerpt(&exact), "x".repeat(1_024));
    let long = vec![b'y'; 1_025];
    assert_eq!(
        compact_response_excerpt(&long),
        format!("{}… (1025 bytes total)", "y".repeat(1_024))
    );
    // A multi-byte character split by the cap decodes lossily instead of failing.
    let mut split = vec![b'z'; 1_023];
    split.extend_from_slice("é".as_bytes());
    assert!(compact_response_excerpt(&split).ends_with("(1025 bytes total)"));
}

#[test]
fn generated_unexpected_status_keeps_message_and_request_id() -> Result<(), GitHubError> {
    let error = generated_error::<()>(generated::Error::UnexpectedStatus {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        headers: request_id_headers("ABCD:1234"),
        body: Bytes::from_static(br#"{"message":"Head sha can't be blank"}"#),
    });
    match error {
        GitHubError::Status {
            status,
            message,
            request_id,
        } => {
            assert_eq!(status, 422);
            assert_eq!(
                message,
                "Head sha can't be blank (GitHub request ID: ABCD:1234)"
            );
            assert_eq!(request_id.as_deref(), Some("ABCD:1234"));
        },
        other => return Err(unexpected("Status", &other)),
    }

    // Without the header there is no request id and no suffix.
    let error = generated_error::<()>(generated::Error::UnexpectedStatus {
        status: StatusCode::BAD_GATEWAY,
        headers: HeaderMap::new(),
        body: Bytes::from_static(b"upstream  down"),
    });
    assert!(matches!(
        error,
        GitHubError::Status { status: 502, ref message, request_id: None } if message == "upstream down"
    ));
    Ok(())
}

#[test]
fn generated_api_error_reports_the_typed_body_and_request_id() {
    let error = generated_error(generated::Error::Api(generated::ResponseValue::new(
        StatusCode::FORBIDDEN,
        request_id_headers("REQ-9"),
        "forbidden".to_string(),
    )));
    assert!(matches!(
        error,
        GitHubError::Status { status: 403, ref message, ref request_id }
            if message == "\"forbidden\" (GitHub request ID: REQ-9)"
                && request_id.as_deref() == Some("REQ-9")
    ));
}

#[test]
fn generated_decode_error_names_the_path_and_truncation() {
    let error = generated_error::<()>(generated::Error::Decode {
        path: "head.sha".to_string(),
        body: Bytes::from_static(b"{ \"head\":\n  {} }"),
        truncated: true,
    });
    assert!(matches!(
        error,
        GitHubError::Decode(ref message)
            if message == "field `head.sha` could not be decoded (response was truncated); response: { \"head\": {} }"
    ));
    let error = generated_error::<()>(generated::Error::Decode {
        path: "id".to_string(),
        body: Bytes::new(),
        truncated: false,
    });
    assert!(matches!(
        error,
        GitHubError::Decode(ref message) if message == "field `id` could not be decoded; response: "
    ));
}

#[test]
fn generated_request_construction_maps_to_request() {
    assert!(matches!(
        generated_error::<()>(generated::Error::request_message("bad base")),
        GitHubError::Request(ref message)
            if message.starts_with("generated request construction failed:")
    ));
}

/// The failure a test returns when a call produced the wrong outcome.
fn unexpected(expected: &str, actual: &dyn std::fmt::Debug) -> GitHubError {
    GitHubError::Request(format!("expected {expected}, got {actual:?}"))
}

/// One request as the loopback listener received it.
#[derive(Debug, Default)]
struct Received {
    request_line: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Received {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// A canned HTTP response: status line tail, extra headers, body.
type Canned = (&'static str, Vec<(&'static str, &'static str)>, String);

/// Serves `responses` in order, one connection each, and returns what it read.
fn serve(
    responses: Vec<Canned>,
) -> std::io::Result<(String, JoinHandle<std::io::Result<Vec<Received>>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let base = format!("http://{}", listener.local_addr()?);
    let handle = std::thread::spawn(move || {
        let mut received = Vec::new();
        for (status, headers, body) in responses {
            let (mut stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(Duration::from_secs(10)))?;
            let mut reader = BufReader::new(stream.try_clone()?);
            let mut request = Received::default();
            reader.read_line(&mut request.request_line)?;
            request.request_line = request.request_line.trim_end().to_string();
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line)?;
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                if let Some((key, value)) = line.split_once(':') {
                    let key = key.trim().to_ascii_lowercase();
                    let value = value.trim().to_string();
                    if key == "content-length" {
                        length = value.parse().unwrap_or(0);
                    }
                    request.headers.push((key, value));
                }
            }
            let mut body_bytes = vec![0; length];
            reader.read_exact(&mut body_bytes)?;
            request.body = String::from_utf8_lossy(&body_bytes).into_owned();
            received.push(request);

            let mut response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
                body.len()
            );
            for (key, value) in headers {
                response.push_str(&format!("{key}: {value}\r\n"));
            }
            response.push_str("\r\n");
            response.push_str(&body);
            stream.write_all(response.as_bytes())?;
            stream.flush()?;
        }
        Ok(received)
    });
    Ok((base, handle))
}

fn io_error(error: std::io::Error) -> GitHubError {
    GitHubError::Transport(error.to_string())
}

const ISSUE_JSON: &str = r#"{
    "number": 7,
    "title": "Created",
    "state": "open",
    "user": { "id": 42, "login": "octocat" },
    "created_at": "2026-01-01T00:00:00Z",
    "updated_at": "2026-01-01T00:00:00Z",
    "comments": 3
}"#;

#[tokio::test]
async fn loopback_round_trips_a_read_a_write_and_error_statuses() -> Result<(), GitHubError> {
    let (base, server) = serve(vec![
        (
            "200 OK",
            vec![],
            r#"{"total_count":51,"workflows":[]}"#.to_string(),
        ),
        ("201 Created", vec![], ISSUE_JSON.to_string()),
        (
            "404 Not Found",
            vec![("x-github-request-id", "F00D:BEEF")],
            r#"{"message":"Not Found","documentation_url":"https://docs.github.com/rest"}"#
                .to_string(),
        ),
        ("200 OK", vec![], "{ \"number\":\n \"seven\" }".to_string()),
    ])
    .map_err(io_error)?;
    let client = GitHubClient::with_auth_at(
        Some(SecretString::from("test-token".to_string())),
        AuthSource::Explicit,
        &base,
    )?;
    let repository = repository();

    // A read: the query carries the page size, and the total drives `next_page`.
    let workflows = client.workflows(&repository, 1).await?;
    assert!(workflows.items.is_empty());
    assert_eq!(workflows.page, 1);
    assert_eq!(workflows.next_page, Some(2));
    assert_eq!(workflows.total_count, Some(51));

    // A write: the JSON body reaches the wire and the 201 decodes.
    let created = client
        .create_issue(
            &repository,
            &NewIssue {
                title: "Created".to_string(),
                body: "body text".to_string(),
                ..NewIssue::default()
            },
        )
        .await?;
    assert_eq!(created.summary.number, 7);
    assert_eq!(created.comments, 3);

    // A non-success status keeps GitHub's message, docs link, and request id.
    match client.issue(&repository, 404).await {
        Err(GitHubError::Status {
            status,
            message,
            request_id,
        }) => {
            assert_eq!(status, 404);
            assert_eq!(
                message,
                "Not Found Documentation: https://docs.github.com/rest (GitHub request ID: F00D:BEEF)"
            );
            assert_eq!(request_id.as_deref(), Some("F00D:BEEF"));
        },
        other => return Err(unexpected("Status", &other)),
    }

    // A success whose body breaks the schema is a Decode error with an excerpt.
    match client.issue(&repository, 8).await {
        Err(GitHubError::Decode(message)) => {
            assert!(
                message.ends_with("; response: { \"number\": \"seven\" }"),
                "{message}"
            );
        },
        other => return Err(unexpected("Decode", &other)),
    }

    let received = server
        .join()
        .map_err(|_| GitHubError::Transport("loopback server panicked".to_string()))?
        .map_err(io_error)?;
    assert_eq!(received.len(), 4);
    assert_eq!(
        received[0].request_line,
        "GET /repos/getkono/karet/actions/workflows?per_page=50&page=1 HTTP/1.1"
    );
    assert_eq!(
        received[0].header("authorization"),
        Some("Bearer test-token")
    );
    assert_eq!(
        received[0].header("x-github-api-version"),
        Some(API_VERSION)
    );
    assert_eq!(received[0].header("user-agent"), Some("karet"));
    assert_eq!(
        received[1].request_line,
        "POST /repos/getkono/karet/issues HTTP/1.1"
    );
    let posted: serde_json::Value = serde_json::from_str(&received[1].body)
        .map_err(|error| GitHubError::Decode(error.to_string()))?;
    assert_eq!(posted["title"], "Created");
    assert_eq!(posted["body"], "body text");
    assert_eq!(
        received[2].request_line,
        "GET /repos/getkono/karet/issues/404 HTTP/1.1"
    );
    Ok(())
}
