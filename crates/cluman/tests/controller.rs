//! Integration tests for the cluman controller — HTTP contract, file I/O, and
//! error propagation.
//!
//! ## Mocking strategy
//!
//! | Boundary | Tool | Purpose |
//! |---|---|---|
//! | Server `/api/push-task` endpoint | [`httpmock::MockServer`] | Intercepts outbound `minreq` calls without a live cluster. |
//! | Compose files on disk | [`tempfile::TempDir`] | Isolates each test from the real filesystem. |
//!
//! ## Exports required
//!
//! The following changes are needed before this file will compile:
//!
//! **`crates/cluman/src/lib.rs`** — add:
//! ```rust,ignore
//! pub mod controller;
//! ```
//!
//! **`crates/cluman/src/controller.rs`** — change visibility:
//! ```rust,ignore
//! pub struct ControllerArgs { ... }       // was pub(crate)
//! pub async fn run_controller(...) { ... } // was pub(crate)
//! ```

use std::fs;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use clap::Parser;
use cluman::controller::{ControllerArgs, run_controller};
use cluman::schemas::Task;
use httpmock::HttpMockResponse;
use httpmock::prelude::*;
use serde_json::json;
use tempfile::TempDir;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Parse `ControllerArgs` from a slice of string-like tokens.
///
/// Panics if clap rejects the arguments — use `ControllerArgs::try_parse_from`
/// directly in tests that verify CLI validation behaviour.
fn parse_args(tokens: impl IntoIterator<Item: Into<std::ffi::OsString> + Clone>) -> ControllerArgs {
    ControllerArgs::try_parse_from(tokens).expect("valid ControllerArgs")
}

/// Write `content` to `<dir>/<filename>` and return the absolute path as a
/// `String` suitable for passing as a clap positional argument.
fn write_compose(dir: &TempDir, filename: &str, content: &str) -> String {
    let path = dir.path().join(filename);
    fs::write(&path, content).unwrap();
    path.to_string_lossy().into_owned()
}

/// Allocate an ephemeral OS port and immediately release it.
///
/// Used to obtain a port number that is very likely to be unreachable when the
/// test needs a connection that will fail.  The tiny TOCTOU race is acceptable.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind to :0")
        .local_addr()
        .expect("local_addr")
        .port()
}

/// Shared body-capture helper.
///
/// Returns an `Arc<Mutex<Vec<Vec<u8>>>>` and a `body_fn` closure suitable for
/// `Then::body_fn`.  Each invocation of the closure appends the incoming
/// request body to the shared vec and returns a fixed 201 JSON response.
///
/// ```rust,ignore
/// let (captured, capture_fn) = make_capture();
/// server.mock_async(|when, then| {
///     when.method(POST).path("/api/push-task");
///     then.status(201).body_fn(capture_fn);
/// }).await;
/// ```
fn make_capture() -> (
    Arc<Mutex<Vec<Vec<u8>>>>,
    impl Fn(&httpmock::HttpMockRequest) -> HttpMockResponse + Send + Sync + 'static,
) {
    let store: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = Arc::clone(&store);
    let f = move |req: &httpmock::HttpMockRequest| {
        cap.lock().unwrap().push(req.body().to_vec());
        HttpMockResponse::builder()
            .status(201)
            .body(b"{\"response\":\"Task queued\"}".as_ref())
            .build()
    };
    (store, f)
}

// ── Wire format ───────────────────────────────────────────────────────────────

/// The controller must set `Content-Type: application/json` on every push.
#[tokio::test]
async fn push_task_sets_content_type_application_json() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let file = write_compose(&dir, "web.yml", "image: nginx");

    let mock = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/api/push-task")
                .header("Content-Type", "application/json");
            then.status(201)
                .json_body(json!({"response":"Task queued"}));
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        &file,
    ]);

    run_controller(args).await.expect("should succeed");
    mock.assert_async().await;
}

/// The controller must POST to `/api/push-task` — not any other path.
#[tokio::test]
async fn push_task_targets_api_push_task_path() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let file = write_compose(&dir, "app.yml", "");

    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.status(201)
                .json_body(json!({"response":"Task queued"}));
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        &file,
    ]);

    run_controller(args).await.expect("should succeed");
    mock.assert_async().await;
}

/// The request body must deserialise as a valid [`Task`].
#[tokio::test]
async fn request_body_deserializes_as_task() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let content = "services:\n  app:\n    image: myapp:1.0\n";
    let file = write_compose(&dir, "app.yml", content);

    let (captured, capture_fn) = make_capture();
    server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.respond_with(capture_fn);
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        &file,
    ]);

    run_controller(args).await.expect("should succeed");

    let bodies = captured.lock().unwrap();
    assert_eq!(
        bodies.len(),
        1,
        "expected exactly one request body captured"
    );
    let task: Task =
        serde_json::from_slice(&bodies[0]).expect("request body must deserialise as Task");
    assert_eq!(task.content, content);
}

/// The `filename` field in the pushed `Task` must be the file's basename, not
/// the full absolute path that the controller reads from disk.
#[tokio::test]
async fn task_filename_is_basename_not_full_path() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let file = write_compose(&dir, "postgres.yml", "image: postgres:16");

    let (captured, capture_fn) = make_capture();
    server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.respond_with(capture_fn);
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        &file,
    ]);

    run_controller(args).await.expect("should succeed");

    let bodies = captured.lock().unwrap();
    let task: Task = serde_json::from_slice(&bodies[0]).unwrap();
    assert_eq!(
        task.filename, "postgres.yml",
        "filename must be the basename only"
    );
    assert!(
        !task.filename.contains('/'),
        "filename must not contain path separators; got: {}",
        task.filename
    );
}

/// File content must reach the server byte-for-byte, including whitespace and
/// newlines.
#[tokio::test]
async fn task_content_is_preserved_verbatim() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let content = "services:\n  db:\n    image: postgres:16\n    environment:\n      POSTGRES_PASSWORD: secret\n";
    let file = write_compose(&dir, "db.yml", content);

    let (captured, capture_fn) = make_capture();
    server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.respond_with(capture_fn);
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        &file,
    ]);

    run_controller(args).await.expect("should succeed");

    let bodies = captured.lock().unwrap();
    let task: Task = serde_json::from_slice(&bodies[0]).unwrap();
    assert_eq!(
        task.content, content,
        "content must be preserved byte-for-byte"
    );
}

/// An empty compose file must be accepted without errors and its empty content
/// must be forwarded to the server.
#[tokio::test]
async fn empty_compose_file_is_pushed_successfully() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let file = write_compose(&dir, "empty.yml", "");

    let (captured, capture_fn) = make_capture();
    server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.respond_with(capture_fn);
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        &file,
    ]);

    run_controller(args)
        .await
        .expect("empty file must be pushed without error");

    let bodies = captured.lock().unwrap();
    let task: Task = serde_json::from_slice(&bodies[0]).unwrap();
    assert_eq!(
        task.content, "",
        "empty file must produce empty task content"
    );
}

/// The pushed body must be accepted by the same deserialisation code the server
/// route uses — this acts as a round-trip contract test.
#[tokio::test]
async fn pushed_body_is_compatible_with_server_push_task_handler() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let content = "services:\n  cache:\n    image: redis:7\n";
    let file = write_compose(&dir, "redis.yml", content);

    let (captured, capture_fn) = make_capture();
    server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.respond_with(capture_fn);
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        &file,
    ]);

    run_controller(args).await.expect("should succeed");

    let bodies = captured.lock().unwrap();
    // Deserialise exactly as the server's POST /api/push-task handler would.
    let task: Task = serde_json::from_slice(&bodies[0])
        .expect("controller payload must be parseable by the server route");
    assert_eq!(task.filename, "redis.yml");
    assert_eq!(task.content, content);
}

// ── Multiple files ────────────────────────────────────────────────────────────

/// One POST per compose file.
#[tokio::test]
async fn each_compose_file_generates_one_push_request() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let f1 = write_compose(&dir, "alpha.yml", "a");
    let f2 = write_compose(&dir, "beta.yml", "b");
    let f3 = write_compose(&dir, "gamma.yml", "c");

    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.status(201)
                .json_body(json!({"response":"Task queued"}));
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        &f1,
        &f2,
        &f3,
    ]);

    run_controller(args).await.expect("should succeed");
    mock.assert_calls_async(3).await;
}

/// Each file is pushed with its own content — no cross-contamination.
#[tokio::test]
async fn each_file_is_pushed_with_its_own_distinct_content() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let f1 = write_compose(&dir, "svc-a.yml", "image: service-a:1.0");
    let f2 = write_compose(&dir, "svc-b.yml", "image: service-b:2.0");

    let (captured, capture_fn) = make_capture();
    server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.respond_with(capture_fn);
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        &f1,
        &f2,
    ]);

    run_controller(args).await.expect("should succeed");

    let bodies = captured.lock().unwrap();
    assert_eq!(bodies.len(), 2, "expected two captured request bodies");

    let tasks: Vec<Task> = bodies
        .iter()
        .map(|b| serde_json::from_slice(b).expect("body is a valid Task"))
        .collect();

    let contents: std::collections::HashSet<&str> =
        tasks.iter().map(|t| t.content.as_str()).collect();
    assert!(
        contents.contains("image: service-a:1.0"),
        "service-a content must appear"
    );
    assert!(
        contents.contains("image: service-b:2.0"),
        "service-b content must appear"
    );
}

/// Basenames of all pushed files must appear in the task filenames.
#[tokio::test]
async fn all_file_basenames_appear_in_pushed_tasks() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let f1 = write_compose(&dir, "web.yml", "");
    let f2 = write_compose(&dir, "db.yml", "");
    let f3 = write_compose(&dir, "cache.yml", "");

    let (captured, capture_fn) = make_capture();
    server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.respond_with(capture_fn);
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        &f1,
        &f2,
        &f3,
    ]);

    run_controller(args).await.expect("should succeed");

    let bodies = captured.lock().unwrap();
    let filenames: std::collections::HashSet<String> = bodies
        .iter()
        .map(|b| {
            let t: Task = serde_json::from_slice(b).unwrap();
            t.filename
        })
        .collect();

    for name in &["web.yml", "db.yml", "cache.yml"] {
        assert!(
            filenames.contains(*name),
            "expected filename {name} in pushed tasks; got: {filenames:?}"
        );
    }
}

/// Filenames must never contain path separators regardless of how deep the
/// source file lives in the directory tree.
#[tokio::test]
async fn filenames_never_contain_path_separators() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();

    // Nested subdirectory.
    let sub = dir.path().join("nested").join("deep");
    fs::create_dir_all(&sub).unwrap();
    let file = sub.join("service.yml");
    fs::write(&file, "image: nested:1").unwrap();

    let (captured, capture_fn) = make_capture();
    server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.respond_with(capture_fn);
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        file.to_str().unwrap(),
    ]);

    run_controller(args).await.expect("should succeed");

    let bodies = captured.lock().unwrap();
    let task: Task = serde_json::from_slice(&bodies[0]).unwrap();
    assert!(
        !task.filename.contains('/'),
        "filename must not contain '/'; got: {}",
        task.filename
    );
    assert!(
        !task.filename.contains('\\'),
        "filename must not contain '\\\\'; got: {}",
        task.filename
    );
    assert_eq!(task.filename, "service.yml");
}

// ── Server response handling ──────────────────────────────────────────────────

/// HTTP 201 is the success status the server returns; `run_controller` must
/// return `Ok(())`.
#[tokio::test]
async fn server_201_response_returns_ok() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let file = write_compose(&dir, "ok.yml", "image: ok");

    server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.status(201)
                .json_body(json!({"response":"Task queued"}));
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        &file,
    ]);

    assert!(
        run_controller(args).await.is_ok(),
        "201 response must produce Ok"
    );
}

/// Any 4xx response from the server means the task was rejected.
/// `run_controller` must return `Err`.
#[tokio::test]
async fn server_400_response_returns_err() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let file = write_compose(&dir, "bad.yml", "");

    server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.status(400).body("Bad Request");
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        &file,
    ]);

    assert!(
        run_controller(args).await.is_err(),
        "400 response must produce Err"
    );
}

/// A 5xx server error must also be treated as failure.
#[tokio::test]
async fn server_500_response_returns_err() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let file = write_compose(&dir, "fail.yml", "");

    server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.status(500).body("Internal Server Error");
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        &file,
    ]);

    assert!(
        run_controller(args).await.is_err(),
        "500 response must produce Err"
    );
}

/// A server that does not respond (connection refused) must be treated as
/// failure.
#[tokio::test]
async fn unreachable_server_returns_err() {
    // Grab a port, release it immediately so it is no longer listening.
    let dead_port = free_port();

    let dir = TempDir::new().unwrap();
    let file = write_compose(&dir, "app.yml", "");

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &dead_port.to_string(),
        &file,
    ]);

    assert!(
        run_controller(args).await.is_err(),
        "unreachable server must produce Err"
    );
}

/// A compose file that does not exist on disk must produce `Err`.
#[tokio::test]
async fn nonexistent_compose_file_returns_err() {
    let server = MockServer::start_async().await;

    // The push-task mock is present but must never be called.
    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.status(201)
                .json_body(json!({"response":"Task queued"}));
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        "/tmp/this-file-absolutely-does-not-exist-cluman-test-abc123.yml",
    ]);

    assert!(
        run_controller(args).await.is_err(),
        "missing compose file must produce Err"
    );
    // The server must not have been contacted for a nonexistent file.
    mock.assert_calls_async(0).await;
}

// ── Error isolation ───────────────────────────────────────────────────────────

/// When one file is missing and another exists, the existing file must still
/// be pushed to the server.  The overall result is still `Err` because at
/// least one file failed, but the good file must not be silently dropped.
#[tokio::test]
async fn missing_file_does_not_prevent_other_files_from_being_pushed() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let good_file = write_compose(&dir, "good.yml", "image: ok");

    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.status(201)
                .json_body(json!({"response":"Task queued"}));
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        // First file does not exist — controller must skip and continue.
        "/tmp/no-such-file-cluman-test-xyz987.yml",
        &good_file,
    ]);

    // Overall result must be Err (one file failed).
    assert!(
        run_controller(args).await.is_err(),
        "overall result must be Err when at least one file fails"
    );

    // The good file must still have been pushed.
    mock.assert_calls_async(1).await;
}

/// A single 4xx rejection among multiple otherwise-successful pushes must
/// cause `run_controller` to return `Err`.
#[tokio::test]
async fn server_4xx_rejection_among_successes_returns_err() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let f1 = write_compose(&dir, "good-a.yml", "ok-a");
    let f2 = write_compose(&dir, "bad.yml", "rejected");
    let f3 = write_compose(&dir, "good-b.yml", "ok-b");

    // Return 400 for the second request, 201 for all others.
    let call_count: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let counter = Arc::clone(&call_count);
    server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.respond_with(move |_req: &httpmock::HttpMockRequest| {
                let mut n = counter.lock().unwrap();
                *n += 1;
                if *n == 2 {
                    HttpMockResponse::builder()
                        .status(400)
                        .body(b"Bad Request".as_ref())
                        .build()
                } else {
                    HttpMockResponse::builder()
                        .status(201)
                        .body(b"{\"response\":\"Task queued\"}".as_ref())
                        .build()
                }
            });
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        &f1,
        &f2,
        &f3,
    ]);

    assert!(
        run_controller(args).await.is_err(),
        "a single 400 among multiple pushes must cause Err"
    );
}

/// All compose files must be attempted even when one push is rejected —
/// the controller must not short-circuit on the first error.
#[tokio::test]
async fn all_files_attempted_even_when_one_push_is_rejected() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let f1 = write_compose(&dir, "first.yml", "first");
    let f2 = write_compose(&dir, "second.yml", "second");
    let f3 = write_compose(&dir, "third.yml", "third");

    // Return 400 for the second push, 201 for the others.
    let call_count: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let counter = Arc::clone(&call_count);
    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.respond_with(move |_req: &httpmock::HttpMockRequest| {
                let mut n = counter.lock().unwrap();
                *n += 1;
                if *n == 2 {
                    HttpMockResponse::builder()
                        .status(400)
                        .body(b"Bad Request".as_ref())
                        .build()
                } else {
                    HttpMockResponse::builder()
                        .status(201)
                        .body(b"{\"response\":\"Task queued\"}".as_ref())
                        .build()
                }
            });
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        &f1,
        &f2,
        &f3,
    ]);

    // Overall result is Err (one 400), but all 3 files must have been attempted.
    let _ = run_controller(args).await;
    mock.assert_calls_async(3).await;
}

// ── All-success ───────────────────────────────────────────────────────────────

/// When every push succeeds, `run_controller` must return `Ok(())`.
#[tokio::test]
async fn all_pushes_succeed_returns_ok() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();

    let files: Vec<String> = (1..=5u8)
        .map(|i| {
            write_compose(
                &dir,
                &format!("svc{i}.yml"),
                &format!("image: svc{i}:latest"),
            )
        })
        .collect();

    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.status(201)
                .json_body(json!({"response":"Task queued"}));
        })
        .await;

    let mut tokens: Vec<String> = vec![
        "controller".into(),
        "--servers".into(),
        "127.0.0.1".into(),
        "--port".into(),
        server.port().to_string(),
    ];
    tokens.extend(files);

    let args = ControllerArgs::try_parse_from(tokens).unwrap();
    assert!(
        run_controller(args).await.is_ok(),
        "all successful pushes must return Ok"
    );
    mock.assert_calls_async(5).await;
}

/// Zero errors and zero files produces `Ok` immediately (nothing to push).
/// This is an edge case but must not panic.
#[tokio::test]
async fn no_files_is_an_accepted_cli_error() {
    // clap requires at least one positional file argument, so no-files should
    // be rejected at parse time rather than causing a runtime panic.
    let result = ControllerArgs::try_parse_from(["controller", "--servers", "127.0.0.1"]);
    assert!(
        result.is_err(),
        "clap must reject invocation with no compose files"
    );
}

// ── IP range resolution ───────────────────────────────────────────────────────

/// A single-IP `--servers` argument expands to exactly one target.
/// Verified by counting the HTTP requests received by the mock.
#[tokio::test]
async fn single_ip_produces_one_request_per_file() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let file = write_compose(&dir, "app.yml", "image: app");

    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.status(201)
                .json_body(json!({"response":"Task queued"}));
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        &server.port().to_string(),
        &file,
    ]);

    run_controller(args).await.expect("should succeed");
    mock.assert_calls_async(1).await;
}

/// A CIDR `/32` (single-host subnet) must resolve to exactly one push per file.
#[tokio::test]
async fn cidr_slash32_resolves_to_one_host() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let file = write_compose(&dir, "one.yml", "image: one");

    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.status(201)
                .json_body(json!({"response":"Task queued"}));
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1/32",
        "--port",
        &server.port().to_string(),
        &file,
    ]);

    run_controller(args).await.expect("should succeed");
    // /32 must expand to exactly one host (127.0.0.1 itself).
    mock.assert_calls_async(1).await;
}

/// A dash range with identical start and end (`A-A`) resolves to one host.
#[tokio::test]
async fn dash_range_same_start_and_end_resolves_to_one_host() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();
    let file = write_compose(&dir, "stack.yml", "image: stack");

    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.status(201)
                .json_body(json!({"response":"Task queued"}));
        })
        .await;

    let args = parse_args([
        "controller",
        "--servers",
        "127.0.0.1-127.0.0.1",
        "--port",
        &server.port().to_string(),
        &file,
    ]);

    run_controller(args).await.expect("should succeed");
    mock.assert_calls_async(1).await;
}

/// N files × 1 server produces exactly N requests — verifying the
/// file × server cross-product.
#[tokio::test]
async fn n_files_times_one_server_produces_n_requests() {
    let server = MockServer::start_async().await;
    let dir = TempDir::new().unwrap();

    const N: u8 = 4;
    let files: Vec<String> = (1..=N)
        .map(|i| write_compose(&dir, &format!("f{i}.yml"), &format!("content-{i}")))
        .collect();

    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/api/push-task");
            then.status(201)
                .json_body(json!({"response":"Task queued"}));
        })
        .await;

    let mut tokens: Vec<String> = vec![
        "controller".into(),
        "--servers".into(),
        "127.0.0.1".into(),
        "--port".into(),
        server.port().to_string(),
    ];
    tokens.extend(files);

    let args = ControllerArgs::try_parse_from(tokens).unwrap();
    run_controller(args).await.expect("should succeed");
    mock.assert_calls_async(N as usize).await;
}

// ── CLI validation ────────────────────────────────────────────────────────────

/// `--servers` is required; omitting it must be a clap error.
#[test]
fn missing_servers_flag_is_cli_error() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("x.yml");
    fs::write(&path, "").unwrap();

    let result = ControllerArgs::try_parse_from(["controller", path.to_str().unwrap()]);
    assert!(result.is_err(), "clap must reject missing --servers");
}

/// An invalid IP in `--servers` must be a clap error.
#[test]
fn invalid_ip_in_servers_is_cli_error() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("x.yml");
    fs::write(&path, "").unwrap();

    let result = ControllerArgs::try_parse_from([
        "controller",
        "--servers",
        "not-an-ip",
        path.to_str().unwrap(),
    ]);
    assert!(result.is_err(), "clap must reject invalid IP in --servers");
}

/// A reversed dash range (end < start) must be rejected by the `IpRange`
/// parser, surfacing as a clap error.
#[test]
fn reversed_dash_range_is_cli_error() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("x.yml");
    fs::write(&path, "").unwrap();

    let result = ControllerArgs::try_parse_from([
        "controller",
        "--servers",
        "10.0.0.20-10.0.0.1",
        path.to_str().unwrap(),
    ]);
    assert!(
        result.is_err(),
        "reversed dash range must be rejected at parse time"
    );
}

/// Multiple `--servers` values separated by commas are all accepted.
#[test]
fn comma_separated_servers_are_accepted_by_clap() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("x.yml");
    fs::write(&path, "").unwrap();

    let result = ControllerArgs::try_parse_from([
        "controller",
        "--servers",
        "10.0.0.1,10.0.0.2,10.0.0.3",
        path.to_str().unwrap(),
    ]);
    assert!(
        result.is_ok(),
        "comma-separated IPs must be accepted by clap"
    );
}

/// The `--port` flag accepts any valid `u16` value.
#[test]
fn custom_port_is_accepted_by_clap() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("x.yml");
    fs::write(&path, "").unwrap();

    let result = ControllerArgs::try_parse_from([
        "controller",
        "--servers",
        "10.0.0.1",
        "--port",
        "8080",
        path.to_str().unwrap(),
    ]);
    assert!(result.is_ok(), "--port 8080 must be accepted");
}

/// Port 0 is technically valid as a `u16` and should be accepted by the parser
/// even though it has no practical meaning for the controller.
#[test]
fn port_zero_is_accepted_by_clap() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("x.yml");
    fs::write(&path, "").unwrap();

    let result = ControllerArgs::try_parse_from([
        "controller",
        "--servers",
        "127.0.0.1",
        "--port",
        "0",
        path.to_str().unwrap(),
    ]);
    assert!(
        result.is_ok(),
        "port 0 is a valid u16 and must not be rejected at parse time"
    );
}
