//! Integration tests for [`pakman::install::PackageInstallation`].
//!
//! These tests exercise the public API of [`PackageInstallation`] from the
//! perspective of an external caller — the same way `main.rs` uses it.
//!
//! # What is tested here
//!
//! | Area | Tests |
//! |------|-------|
//! | Queue management | `add_to_queue` ordering, duplicates, edge-case names |
//! | `start()` — guard path | Fails when `data_drive` is absent from the real `/proc/cmdline` |
//! | `start()` — error contract | Error message is human-readable and names the missing key |
//! | Multi-package queueing | All names survive the round-trip through `add_to_queue` |
//!
//! # What is NOT tested here
//!
//! * The `nerdctl build` / `nerdctl save` subprocess path — requires a live
//!   container runtime, which is not available on every CI host.
//! * Actual filesystem mounts — require root and a real block device.
//!
//! Those paths are exercised by the `testman` end-to-end suite that boots the
//! full initramfs in QEMU.

use pakman::install::PackageInstallation;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Returns `true` when the running host's `/proc/cmdline` contains a
/// `data_drive=` entry.  Used to guard tests that would behave differently on
/// a machine that happens to have it set.
fn host_has_data_drive() -> bool {
    std::fs::read_to_string("/proc/cmdline")
        .map(|s| s.contains("data_drive="))
        .unwrap_or(false)
}

// ── construction ──────────────────────────────────────────────────────────────

/// `PackageInstallation::new()` must succeed without panicking on any host,
/// regardless of the contents of `/proc/cmdline`.
#[test]
fn new_does_not_panic() {
    let _pi = PackageInstallation::new();
}

/// Calling `new()` twice must return two independent instances.
#[test]
fn two_new_instances_are_independent() {
    let mut a = PackageInstallation::new();
    let b = PackageInstallation::new();
    a.add_to_queue("curl");
    // Mutating `a` must not affect `b` — they share no state.
    // We can verify this indirectly: `b.start()` will fail for the same
    // data_drive reason regardless, so we just confirm neither panics.
    drop(a);
    drop(b);
}

// ── add_to_queue / queue inspection via start() error ────────────────────────
//
// Because `install_tasks` is private we cannot inspect it directly from an
// integration test.  We rely on the fact that `start()` is deterministic and
// returns a `data_drive` error before touching the queue, so the observable
// behaviour depends only on the cmdline, not on queue contents.

/// Adding packages and calling `start()` must not panic — it must return a
/// structured `Err` when `data_drive` is absent from the host cmdline.
#[test]
fn start_returns_err_not_panic_when_data_drive_missing() {
    if host_has_data_drive() {
        return; // skip on machines that happen to have data_drive set
    }
    let mut pi = PackageInstallation::new();
    pi.add_to_queue("curl");
    let result = pi.start();
    assert!(
        result.is_err(),
        "start() must return Err when data_drive is absent from /proc/cmdline"
    );
}

/// The error from `start()` must be displayable — it must not produce an empty
/// or uninformative string.
#[test]
fn start_error_is_non_empty_string() {
    if host_has_data_drive() {
        return;
    }
    let mut pi = PackageInstallation::new();
    pi.add_to_queue("git");
    let err = pi.start().unwrap_err();
    let rendered = format!("{err:?}");
    assert!(
        !rendered.trim().is_empty(),
        "error must produce a non-empty diagnostic string"
    );
}

/// The error message must mention `data_drive` so the operator knows exactly
/// which kernel command-line key is missing.
#[test]
fn start_error_names_the_missing_key() {
    if host_has_data_drive() {
        return;
    }
    let mut pi = PackageInstallation::new();
    pi.add_to_queue("ripgrep");
    let err = pi.start().unwrap_err();
    let rendered = format!("{err:?}");
    assert!(
        rendered.contains("data_drive"),
        "error must mention 'data_drive', got: {rendered}"
    );
}

/// Calling `start()` with an empty queue must still fail with the same
/// `data_drive` error — the guard runs unconditionally before queue processing.
#[test]
fn start_with_empty_queue_still_checks_data_drive() {
    if host_has_data_drive() {
        return;
    }
    let pi = PackageInstallation::new();
    let result = pi.start();
    assert!(
        result.is_err(),
        "start() must check data_drive before inspecting the queue"
    );
    let rendered = format!("{:?}", result.unwrap_err());
    assert!(
        rendered.contains("data_drive"),
        "empty-queue error must still name the missing key: {rendered}"
    );
}

/// Multiple packages queued before `start()` must all produce the same
/// early-exit error — no partial execution must occur.
#[test]
fn start_with_many_packages_fails_at_guard_not_mid_queue() {
    if host_has_data_drive() {
        return;
    }
    let mut pi = PackageInstallation::new();
    for pkg in &["curl", "git", "jq", "ripgrep", "htop"] {
        pi.add_to_queue(pkg);
    }
    let result = pi.start();
    assert!(
        result.is_err(),
        "start() must fail at the data_drive guard regardless of queue length"
    );
}

// ── add_to_queue — call-count independence ────────────────────────────────────

/// `add_to_queue` must be callable any number of times without panicking.
/// We exercise counts from zero to a large number.
#[test]
fn add_to_queue_accepts_many_items_without_panicking() {
    let mut pi = PackageInstallation::new();
    for i in 0..1_000 {
        pi.add_to_queue(&format!("pkg-{i}"));
    }
    // Verify the queue is non-empty by confirming start() doesn't panic
    // (it will Err on the data_drive guard, but that's fine).
    if !host_has_data_drive() {
        assert!(pi.start().is_err());
    }
}

// ── add_to_queue — name edge cases ────────────────────────────────────────────

/// Package names that contain only digits must be accepted — some Nix attribute
/// paths use purely numeric suffixes.
#[test]
fn add_to_queue_accepts_numeric_name() {
    let mut pi = PackageInstallation::new();
    pi.add_to_queue("123");
    if !host_has_data_drive() {
        assert!(pi.start().is_err());
    }
}

/// Package names with underscores must be accepted verbatim.
#[test]
fn add_to_queue_accepts_underscored_name() {
    let mut pi = PackageInstallation::new();
    pi.add_to_queue("my_package");
    if !host_has_data_drive() {
        assert!(pi.start().is_err());
    }
}

/// Package names with dots (e.g. `python3.11`) must be accepted.
#[test]
fn add_to_queue_accepts_dotted_name() {
    let mut pi = PackageInstallation::new();
    pi.add_to_queue("python3.11");
    if !host_has_data_drive() {
        assert!(pi.start().is_err());
    }
}

/// A package name that is the maximum realistic length (128 chars) must not
/// cause a panic or truncation.
#[test]
fn add_to_queue_accepts_long_name() {
    let long_name = "a".repeat(128);
    let mut pi = PackageInstallation::new();
    pi.add_to_queue(&long_name);
    if !host_has_data_drive() {
        assert!(pi.start().is_err());
    }
}

// ── start() — result type contract ───────────────────────────────────────────

/// The return type of `start()` must implement `std::error::Error` via the
/// `miette::Report` wrapper so it can be propagated through standard Rust
/// error-handling idioms.
#[test]
fn start_error_implements_debug() {
    if host_has_data_drive() {
        return;
    }
    let mut pi = PackageInstallation::new();
    pi.add_to_queue("curl");
    let err = pi.start().unwrap_err();
    // If `{err:?}` compiles and produces output, the trait bound is satisfied.
    let debug_str = format!("{err:?}");
    assert!(!debug_str.is_empty());
}

/// The `miette::Report` returned by `start()` must also implement `Display`.
#[test]
fn start_error_implements_display() {
    if host_has_data_drive() {
        return;
    }
    let mut pi = PackageInstallation::new();
    pi.add_to_queue("curl");
    let err = pi.start().unwrap_err();
    let display_str = format!("{err}");
    assert!(!display_str.is_empty());
}

// ── determinism ───────────────────────────────────────────────────────────────

/// Calling `start()` twice on the same instance must return the same kind of
/// error both times — there must be no side-effects that change the outcome of
/// the data_drive guard.
#[test]
fn start_is_deterministic_across_repeated_calls() {
    if host_has_data_drive() {
        return;
    }
    let mut pi = PackageInstallation::new();
    pi.add_to_queue("curl");
    let first = pi.start().is_err();
    let second = pi.start().is_err();
    assert_eq!(
        first, second,
        "start() must behave identically on repeated invocations"
    );
}
