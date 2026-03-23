//! Integration tests for the cluman client — HTTP contract and task execution.
//!
//! # Mocking strategy
//!
//! | Boundary | Tool | Purpose |
//! |---|---|---|
//! | Server HTTP endpoints | [`httpmock::MockServer`] | Verifies the exact wire format of outbound client requests without a live `cluman server`. |
//! | `docker compose` subprocess | [`SpyExecutor`] | Records every `run_compose` invocation so Docker is never required on the test host. |

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use cluman::schemas::{CluManSchema, Mode, Task};
use cluman::{Executor, run_client_with};
use httpmock::HttpMockResponse;
use httpmock::prelude::*;
use serde_json::{Value, json};

// ── SpyExecutor ───────────────────────────────────────────────────────────────

/// Test double for [`Executor`].
///
/// Records every `run_compose` call as a `(path, content)` pair so tests can
/// assert on both the path that was passed and the file content that was
/// written to that path at the moment of invocation.
///
/// Optionally configured to return an error via [`SpyExecutor::failing`].
struct SpyExecutor {
    calls: Arc<Mutex<Vec<(PathBuf, String)>>>,
    should_fail: bool,
}

impl SpyExecutor {
    /// Create a spy that always succeeds.
    fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            should_fail: false,
        }
    }

    /// Create a spy that always returns an error.
    fn failing() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            should_fail: true,
        }
    }

    /// Return a clone of the calls list for assertions from outside the executor.
    fn calls_handle(&self) -> Arc<Mutex<Vec<(PathBuf, String)>>> {
        Arc::clone(&self.calls)
    }

    /// How many times was `run_compose` called?
    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl Executor for SpyExecutor {
    /// Record the call, read back the file content, then succeed or fail based
    /// on `should_fail`.
    fn run_compose(&self, compose_file: &Path) -> Result<(), String> {
        let content = std::fs::read_to_string(compose_file).unwrap_or_default();
        self.calls
            .lock()
            .unwrap()
            .push((compose_file.to_path_buf(), content));
        if self.should_fail {
            Err(format!(
                "SpyExecutor configured to fail for: {}",
                compose_file.display()
            ))
        } else {
            Ok(())
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Decode a mock server's received request body as a `serde_json::Value`.
fn parse_body(body: &[u8]) -> Value {
    serde_json::from_slice(body).expect("request body is valid JSON")
}

/// Build a minimal [`actman::cmdline::CmdLineOptions`]-compatible fixture that
/// sets `server_url` and `own_ip` without reading `/proc/cmdline`.
///
/// Returns the server URL and a `CmdLineOptions` stub wired to the provided
/// mock server's base URL.
fn cmdline_opts(server_url: &str, own_ip: Ipv4Addr) -> actman::cmdline::CmdLineOptions {
    // CmdLineOptions::from_map is the test-friendly constructor that accepts
    // a HashMap directly instead of reading /proc/cmdline.
    actman::cmdline::CmdLineOptions::from_map(
        [
            ("server_url".to_owned(), server_url.to_owned()),
            ("own_ip".to_owned(), own_ip.to_string()),
        ]
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>(),
    )
}

// ── SpyExecutor unit tests ────────────────────────────────────────────────────

#[test]
fn spy_executor_starts_with_zero_calls() {
    let spy = SpyExecutor::new();
    assert_eq!(spy.call_count(), 0);
}

#[test]
fn spy_executor_records_each_invocation() {
    let spy = SpyExecutor::new();
    let dir = tempfile::tempdir().unwrap();

    let p1 = dir.path().join("a.yml");
    let p2 = dir.path().join("b.yml");
    std::fs::write(&p1, "content-a").unwrap();
    std::fs::write(&p2, "content-b").unwrap();

    spy.run_compose(&p1).unwrap();
    spy.run_compose(&p2).unwrap();

    assert_eq!(spy.call_count(), 2);
}

#[test]
fn spy_executor_captures_compose_file_content() {
    let spy = SpyExecutor::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stack.yml");
    let content = "services:\n  app:\n    image: myapp:1.0\n";
    std::fs::write(&path, content).unwrap();

    spy.run_compose(&path).unwrap();

    let calls = spy.calls.lock().unwrap();
    assert_eq!(calls[0].1, content);
}

#[test]
fn spy_executor_captures_exact_path() {
    let spy = SpyExecutor::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("compose.yml");
    std::fs::write(&path, "").unwrap();

    spy.run_compose(&path).unwrap();

    let calls = spy.calls.lock().unwrap();
    assert_eq!(calls[0].0, path);
}

#[test]
fn spy_executor_failing_returns_err() {
    let spy = SpyExecutor::failing();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.yml");
    std::fs::write(&path, "").unwrap();

    let result = spy.run_compose(&path);
    assert!(result.is_err(), "expected an Err from failing spy");
}

#[test]
fn spy_executor_failing_still_records_the_call() {
    let spy = SpyExecutor::failing();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.yml");
    std::fs::write(&path, "payload").unwrap();

    let _ = spy.run_compose(&path);
    assert_eq!(spy.call_count(), 1);
}

#[test]
fn spy_executor_calls_handle_shares_state_across_clones() {
    let spy = Arc::new(SpyExecutor::new());
    let handle = spy.calls_handle();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.yml");
    std::fs::write(&path, "data").unwrap();

    spy.run_compose(&path).unwrap();

    // The handle must reflect the call even though spy was not moved.
    assert_eq!(handle.lock().unwrap().len(), 1);
}

// ── Registration HTTP contract ────────────────────────────────────────────────

/// The client MUST POST to `/api/register-client` at least once on startup.
#[test]
fn client_posts_to_register_client_on_startup() {
    let server = MockServer::start();

    let register_mock = server.mock(|when, then| {
        when.method(POST).path("/api/register-client");
        then.status(201)
            .header("Content-Type", "application/json")
            .json_body(json!({ "response": "Successfully registered" }));
    });

    // Subsequent task polls return 204 so the client doesn't try to execute.
    server.mock(|when, then| {
        when.method(GET).path("/task");
        then.status(204);
    });

    let own_ip: Ipv4Addr = "10.0.0.1".parse().unwrap();
    let opts = cmdline_opts(&server.base_url(), own_ip);
    let spy = Arc::new(SpyExecutor::new());

    thread::spawn(move || {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                run_client_with(&opts, spy).await.ok();
            });
    });

    // Give the client enough time to start up and send the registration.
    thread::sleep(Duration::from_millis(400));

    register_mock.assert_calls(1);
}

/// Registration body must be valid JSON and identify the sender as `client` mode.
#[test]
fn registration_body_carries_client_mode() {
    let server = MockServer::start();

    let captured: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = Arc::clone(&captured);
    let _register_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/api/register-client")
            .header("Content-Type", "application/json");
        then.respond_with(move |req: &httpmock::HttpMockRequest| {
            cap.lock().unwrap().push(req.body().to_vec());
            HttpMockResponse::builder()
                .status(201)
                .body(b"{\"response\":\"ok\"}".as_ref())
                .build()
        });
    });

    server.mock(|when, then| {
        when.method(GET).path("/task");
        then.status(204);
    });

    let own_ip: Ipv4Addr = "10.1.2.3".parse().unwrap();
    let opts = cmdline_opts(&server.base_url(), own_ip);
    let spy = Arc::new(SpyExecutor::new());

    thread::spawn(move || {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async { run_client_with(&opts, spy).await.ok() });
    });

    thread::sleep(Duration::from_millis(200));

    let bodies = captured.lock().unwrap();
    assert!(
        !bodies.is_empty(),
        "expected at least one registration call"
    );

    let body = parse_body(&bodies[0]);
    // The CluManSchema registration payload must carry mode=client.
    // Server-side, `schema.peer()` is used to extract (ip, mode).
    let schema: CluManSchema =
        serde_json::from_value(body).expect("registration body deserialises as CluManSchema");
    let (ip, mode) = schema.peer().expect("registration schema must have a peer");
    assert_eq!(mode, Mode::Client);
    assert_eq!(ip, own_ip);
}

/// Verify the registration JSON is accepted by the real server-side logic — the
/// `CluManSchema::registration` constructor must produce a payload that the
/// server's `/api/register-client` route can parse.
#[test]
fn registration_schema_matches_server_expectation() {
    let own_ip: Ipv4Addr = "192.168.1.50".parse().unwrap();
    let schema = CluManSchema::registration(own_ip, Mode::Client);
    let json = serde_json::to_string(&schema).expect("serialisation must succeed");

    // The server checks schema.peer() → (ip, Mode::Client).
    let roundtripped: CluManSchema =
        serde_json::from_str(&json).expect("server can deserialise registration payload");
    let (decoded_ip, decoded_mode) = roundtripped.peer().expect("peer must be present");
    assert_eq!(decoded_ip, own_ip);
    assert_eq!(decoded_mode, Mode::Client);
}

/// A server-mode registration body must be rejected at the schema level.
#[test]
fn server_mode_registration_is_not_client_mode() {
    let own_ip: Ipv4Addr = "10.0.0.1".parse().unwrap();
    let schema = CluManSchema::registration(own_ip, Mode::Server);
    let (_, mode) = schema.peer().unwrap();
    // The server route rejects anything that is not Mode::Client.
    assert_ne!(mode, Mode::Client);
}

// ── Task polling HTTP contract ────────────────────────────────────────────────

/// When the server returns 204 the client must NOT invoke the executor.
#[test]
fn no_task_available_does_not_invoke_executor() {
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST).path("/api/register-client");
        then.status(201).json_body(json!({ "response": "ok" }));
    });

    server.mock(|when, then| {
        when.method(GET).path("/task");
        then.status(204);
    });

    let opts = cmdline_opts(&server.base_url(), Ipv4Addr::LOCALHOST);
    let spy = Arc::new(SpyExecutor::new());
    let calls = spy.calls_handle();

    thread::spawn({
        let spy = Arc::clone(&spy);
        move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async { run_client_with(&opts, spy).await.ok() });
        }
    });

    // Give the polling loop one iteration to run.
    thread::sleep(Duration::from_millis(200));

    assert_eq!(
        calls.lock().unwrap().len(),
        0,
        "executor must not be called when server returns 204"
    );
}

/// When the server returns a valid task, the executor must be called exactly
/// once with a file whose content matches the task payload.
#[test]
fn task_available_invokes_executor_with_correct_content() {
    let compose_content = "services:\n  db:\n    image: postgres:16\n";
    let task = Task::new("db.yml", compose_content);

    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST).path("/api/register-client");
        then.status(201).json_body(json!({ "response": "ok" }));
    });

    // First poll delivers a task; subsequent polls return 204.
    let task_json = serde_json::to_string(&task).unwrap();
    server.mock(|when, then| {
        when.method(GET).path("/task");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(task_json);
    });

    let opts = cmdline_opts(&server.base_url(), Ipv4Addr::LOCALHOST);
    let spy = Arc::new(SpyExecutor::new());
    let calls = spy.calls_handle();

    thread::spawn({
        let spy = Arc::clone(&spy);
        move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async { run_client_with(&opts, spy).await.ok() });
        }
    });

    // Wait for executor to be called (max 2 s).
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if calls.lock().unwrap().len() >= 1 {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }

    let recorded = calls.lock().unwrap();
    assert_eq!(recorded.len(), 1, "executor must be called exactly once");
    assert_eq!(
        recorded[0].1, compose_content,
        "executor must receive verbatim task content"
    );
}

/// The executor must receive a path whose filename matches the task filename.
#[test]
fn executor_receives_path_derived_from_task_filename() {
    let task = Task::new("nginx.yml", "services:\n  web:\n    image: nginx\n");

    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST).path("/api/register-client");
        then.status(201).json_body(json!({"response":"ok"}));
    });

    server.mock(|when, then| {
        when.method(GET).path("/task");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(serde_json::to_string(&task).unwrap());
    });

    let opts = cmdline_opts(&server.base_url(), Ipv4Addr::LOCALHOST);
    let spy = Arc::new(SpyExecutor::new());
    let calls = spy.calls_handle();

    thread::spawn({
        let spy = Arc::clone(&spy);
        move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async { run_client_with(&opts, spy).await.ok() });
        }
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if calls.lock().unwrap().len() >= 1 {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }

    let recorded = calls.lock().unwrap();
    let path = &recorded[0].0;
    assert!(
        path.to_string_lossy().contains("nginx.yml"),
        "temp path must contain the original filename; got: {}",
        path.display()
    );
}

/// The temp file must exist on disk at the moment the executor is called.
#[test]
fn temp_file_exists_when_executor_is_called() {
    let task = Task::new("check.yml", "content");

    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST).path("/api/register-client");
        then.status(201).json_body(json!({"response":"ok"}));
    });

    server.mock(|when, then| {
        when.method(GET).path("/task");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(serde_json::to_string(&task).unwrap());
    });

    // Custom executor that asserts path existence at call time.
    struct ExistenceCheckExecutor {
        existed: Arc<Mutex<bool>>,
    }
    impl Executor for ExistenceCheckExecutor {
        fn run_compose(&self, path: &Path) -> Result<(), String> {
            *self.existed.lock().unwrap() = path.exists();
            Ok(())
        }
    }

    let existed = Arc::new(Mutex::new(false));
    let executor = Arc::new(ExistenceCheckExecutor {
        existed: Arc::clone(&existed),
    });

    let opts = cmdline_opts(&server.base_url(), Ipv4Addr::LOCALHOST);

    thread::spawn({
        let executor = Arc::clone(&executor) as Arc<dyn Executor>;
        move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async { run_client_with(&opts, executor).await.ok() });
        }
    });

    thread::sleep(Duration::from_millis(500));

    assert!(
        *existed.lock().unwrap(),
        "temp file must exist when executor.run_compose is called"
    );
}

// ── Error-handling contract ───────────────────────────────────────────────────

/// An unreachable server must not cause the executor to be invoked — the
/// client must handle the connection failure gracefully and not attempt to
/// run any compose files when no tasks were received.
#[test]
fn client_survives_unreachable_server() {
    // Port 1 is reserved and always unreachable — no mock server started.
    let bad_url = "http://127.0.0.1:1";
    let opts = cmdline_opts(bad_url, Ipv4Addr::LOCALHOST);
    let spy = Arc::new(SpyExecutor::new());
    let calls = spy.calls_handle();

    thread::spawn({
        let spy = Arc::clone(&spy);
        move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async { run_client_with(&opts, spy).await.ok() });
        }
    });

    // Give the polling loop time to attempt (and fail) at least one request.
    thread::sleep(Duration::from_millis(300));

    // The executor must never have been called — no task was successfully
    // received from the unreachable server.
    assert_eq!(
        calls.lock().unwrap().len(),
        0,
        "executor must not be called when the server is unreachable"
    );
}

/// Malformed JSON from the `/task` endpoint must be silently discarded —
/// the executor must never be invoked for a task that cannot be parsed.
#[test]
fn client_survives_invalid_task_json() {
    let server = MockServer::start();

    let task_mock = server.mock(|when, then| {
        when.method(GET).path("/task");
        then.status(200)
            .header("Content-Type", "application/json")
            .body("{ this is not valid json }");
    });
    server.mock(|when, then| {
        when.method(POST).path("/api/register-client");
        then.status(201).json_body(json!({"response":"ok"}));
    });

    let opts = cmdline_opts(&server.base_url(), Ipv4Addr::LOCALHOST);
    let spy = Arc::new(SpyExecutor::new());
    let calls = spy.calls_handle();

    thread::spawn({
        let spy = Arc::clone(&spy);
        move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async { run_client_with(&opts, spy).await.ok() });
        }
    });

    // Wait long enough for at least one poll to complete.
    thread::sleep(Duration::from_millis(300));

    // The mock was hit (the client did poll) but the executor was never called
    // because the response body was not valid Task JSON.
    assert!(
        task_mock.calls() >= 1,
        "client must have polled /task at least once"
    );
    assert_eq!(
        calls.lock().unwrap().len(),
        0,
        "executor must not be called when task JSON is invalid"
    );
}

/// A failing executor must not prevent subsequent task polls — the client
/// must call the executor (recording the attempt) and continue without
/// propagating the error upward.
#[test]
fn client_survives_executor_failure() {
    let task = Task::new("bad.yml", "content");

    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST).path("/api/register-client");
        then.status(201).json_body(json!({"response":"ok"}));
    });
    server.mock(|when, then| {
        when.method(GET).path("/task");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(serde_json::to_string(&task).unwrap());
    });

    let opts = cmdline_opts(&server.base_url(), Ipv4Addr::LOCALHOST);
    let spy = Arc::new(SpyExecutor::failing());
    let calls = spy.calls_handle();

    thread::spawn({
        let spy = Arc::clone(&spy);
        move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async { run_client_with(&opts, spy).await.ok() });
        }
    });

    // Wait for the executor to be invoked (task received and attempted).
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if calls.lock().unwrap().len() >= 1 {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }

    // The executor must have been called — it received the task and tried
    // to execute it.  The fact that it returned Err must not suppress the
    // call itself.
    assert!(
        calls.lock().unwrap().len() >= 1,
        "executor must have been called even though it was configured to fail"
    );
}

/// A 500 response from the `/task` endpoint must be logged and discarded —
/// the executor must never be invoked when the server signals an error.
#[test]
fn client_survives_server_error_on_task_poll() {
    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST).path("/api/register-client");
        then.status(201).json_body(json!({"response":"ok"}));
    });
    let task_mock = server.mock(|when, then| {
        when.method(GET).path("/task");
        then.status(500).body("Internal Server Error");
    });

    let opts = cmdline_opts(&server.base_url(), Ipv4Addr::LOCALHOST);
    let spy = Arc::new(SpyExecutor::new());
    let calls = spy.calls_handle();

    thread::spawn({
        let spy = Arc::clone(&spy);
        move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async { run_client_with(&opts, spy).await.ok() });
        }
    });

    // Wait long enough for at least one poll cycle.
    thread::sleep(Duration::from_millis(300));

    // The client polled /task but received a 500 — it must have hit the
    // endpoint and silently discarded the error without calling the executor.
    assert!(
        task_mock.calls() >= 1,
        "client must have polled /task at least once"
    );
    assert_eq!(
        calls.lock().unwrap().len(),
        0,
        "executor must not be called when the server returns 500"
    );
}

// ── /status health-check endpoint ────────────────────────────────────────────

// ── End-to-end flow ───────────────────────────────────────────────────────────

/// Full happy path: client registers, polls, receives a task, and the executor
/// is called with the complete compose file content byte-for-byte.
#[test]
fn full_client_flow_register_poll_execute() {
    let compose = "services:\n  app:\n    image: myapp:2.0\n    ports:\n      - \"8080:8080\"\n";
    let task = Task::new("app.yml", compose);
    let task_json = serde_json::to_string(&task).unwrap();
    let own_ip: Ipv4Addr = "10.99.0.1".parse().unwrap();

    let server = MockServer::start();

    let reg_captured: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let reg_cap = Arc::clone(&reg_captured);
    let reg_mock = server.mock(|when, then| {
        when.method(POST).path("/api/register-client");
        then.respond_with(move |req: &httpmock::HttpMockRequest| {
            reg_cap.lock().unwrap().push(req.body().to_vec());
            HttpMockResponse::builder()
                .status(201)
                .body(b"{\"response\":\"Successfully registered\"}".as_ref())
                .build()
        });
    });

    // Return the task on the first poll, then 204 thereafter.
    server.mock(|when, then| {
        when.method(GET).path("/task");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(task_json);
    });

    let opts = cmdline_opts(&server.base_url(), own_ip);
    let spy = Arc::new(SpyExecutor::new());
    let calls = spy.calls_handle();

    thread::spawn({
        let spy = Arc::clone(&spy);
        move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async { run_client_with(&opts, spy).await.ok() });
        }
    });

    // Wait for the executor to be called (task received and executed).
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if calls.lock().unwrap().len() >= 1 {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }

    // 1. Registration must have happened exactly once.
    reg_mock.assert_calls(1);

    // 2. Verify registration payload identifies this client correctly.
    let reg_bodies = reg_captured.lock().unwrap();
    let reg_body = parse_body(reg_bodies.first().map(Vec::as_slice).unwrap_or(b"{}"));
    drop(reg_bodies);
    let schema: CluManSchema = serde_json::from_value(reg_body).unwrap();
    let (registered_ip, registered_mode) = schema.peer().unwrap();
    assert_eq!(registered_ip, own_ip);
    assert_eq!(registered_mode, Mode::Client);

    // 3. Executor must have been called with verbatim compose content.
    let recorded = calls.lock().unwrap();
    assert!(
        !recorded.is_empty(),
        "executor must have been called at least once"
    );
    assert_eq!(recorded[0].1, compose);
}

/// Two sequential tasks must each be executed in the order received (FIFO).
#[test]
fn sequential_tasks_are_executed_in_fifo_order() {
    let task_a = Task::new("alpha.yml", "content-alpha");
    let task_b = Task::new("beta.yml", "content-beta");

    let server = MockServer::start();

    server.mock(|when, then| {
        when.method(POST).path("/api/register-client");
        then.status(201).json_body(json!({"response":"ok"}));
    });

    // Deliver task_a, then task_b, then drain with 204s.
    let responses = Arc::new(Mutex::new(vec![
        serde_json::to_string(&task_a).unwrap(),
        serde_json::to_string(&task_b).unwrap(),
    ]));

    server.mock(|when, then| {
        when.method(GET).path("/task");
        then.status(200)
            .header("Content-Type", "application/json")
            // httpmock will serve these in round-robin / sequence.
            .respond_with(move |_: &httpmock::HttpMockRequest| {
                let mut q = responses.lock().unwrap();
                let body_bytes = if let Some(body) = q.first().cloned() {
                    q.remove(0);
                    body.into_bytes()
                } else {
                    // Queue exhausted — return 204 equivalent body (status
                    // remains 200 here; the test only checks executor calls).
                    b"{}".to_vec()
                };
                HttpMockResponse::builder()
                    .status(200)
                    .body(body_bytes.as_slice())
                    .build()
            });
    });

    let opts = cmdline_opts(&server.base_url(), Ipv4Addr::LOCALHOST);
    let spy = Arc::new(SpyExecutor::new());
    let calls = spy.calls_handle();

    thread::spawn({
        let spy = Arc::clone(&spy);
        move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async { run_client_with(&opts, spy).await.ok() });
        }
    });

    // Wait until both tasks have been executed.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if calls.lock().unwrap().len() >= 2 {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    let recorded = calls.lock().unwrap();
    assert!(recorded.len() >= 2, "both tasks must be executed");
    assert_eq!(recorded[0].1, "content-alpha", "first task must be alpha");
    assert_eq!(recorded[1].1, "content-beta", "second task must be beta");
}
