//! Benchmarks for the `actman` crate.
//!
//! Covers:
//! * `CmdLineOptions::param_search` — the kernel command-line parser, at
//!   various input sizes and shapes.
//! * `RebootCMD::from` — basename-to-mode dispatch for every variant plus the
//!   full-path and unknown-name fast-paths.
//! * `Preboot::new` / `Preboot::default` — construction (live sysfs probes).

use actman::{cmdline::CmdLineOptions, preboot::Preboot, reboot::RebootCMD};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};

// ── helpers ───────────────────────────────────────────────────────────────────

/// A minimal, realistic kernel command line (no key=value pairs, just flags).
const CMDLINE_BARE_FLAGS: &str = "quiet ro splash";

/// A compact command line with a handful of key=value pairs and one bare flag.
const CMDLINE_SMALL: &str = "console=ttyS0 earlyprintk=ttyS0 quiet net.ifnames=0 biosdevname=0";

/// A realistic medium-length command line resembling what a real initramfs boot
/// might see (server_url, tag, hash, data_drive, own_ip, plus common flags).
const CMDLINE_MEDIUM: &str = "console=ttyS0 earlyprintk=ttyS0 quiet ro net.ifnames=0 biosdevname=0 \
     server_url=http://10.0.0.1:9999 own_ip=10.0.0.42 tag=util-mdl:latest \
     hash=sha256:deadbeefcafe data_drive=/dev/sda2 base_url=registry.example.com/mtos";

/// A large synthetic command line with 64 key=value pairs.
fn large_cmdline() -> String {
    (0..64)
        .map(|i| format!("key{i}=value{i}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A command line where every value itself contains `=` signs (URL-like).
const CMDLINE_VALUES_WITH_EQUALS: &str =
    "url=http://host/path?a=1&b=2 token=abc=def== other=x=y console=ttyS0";

// ── param_search ──────────────────────────────────────────────────────────────

fn bench_param_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("cmdline/param_search");

    // Empty input — exercises the fast-exit path.
    group.bench_function("empty", |b| {
        b.iter(|| CmdLineOptions::param_search(""));
    });

    // Only bare flags — every token is filtered out.
    group.bench_function("bare_flags_only", |b| {
        b.iter(|| CmdLineOptions::param_search(CMDLINE_BARE_FLAGS));
    });

    // Typical small command line.
    group.bench_function("small", |b| {
        b.iter(|| CmdLineOptions::param_search(CMDLINE_SMALL));
    });

    // Realistic medium command line (contains all updman/cluman keys).
    group.bench_function("medium", |b| {
        b.iter(|| CmdLineOptions::param_search(CMDLINE_MEDIUM));
    });

    // Large synthetic line — stresses HashMap growth / reallocation.
    group.bench_function("large_64_pairs", |b| {
        let input = large_cmdline();
        b.iter(|| CmdLineOptions::param_search(&input));
    });

    // Values that themselves contain `=` — exercises the split_once fast-path.
    group.bench_function("values_with_equals", |b| {
        b.iter(|| CmdLineOptions::param_search(CMDLINE_VALUES_WITH_EQUALS));
    });

    // Single key=value token — minimum non-empty case.
    group.bench_function("single_pair", |b| {
        b.iter(|| CmdLineOptions::param_search("console=ttyS0"));
    });

    group.finish();
}

// ── param_search scaling ──────────────────────────────────────────────────────

/// Show how parse time scales with the number of key=value pairs.
fn bench_param_search_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("cmdline/param_search_scaling");

    for n in [1usize, 8, 16, 32, 64, 128] {
        let input: String = (0..n)
            .map(|i| format!("key{i}=value{i}"))
            .collect::<Vec<_>>()
            .join(" ");

        group.bench_with_input(BenchmarkId::from_parameter(n), &input, |b, s| {
            b.iter(|| CmdLineOptions::param_search(s.as_str()));
        });
    }

    group.finish();
}

// ── RebootCMD dispatch ────────────────────────────────────────────────────────

fn bench_reboot_cmd_dispatch(c: &mut Criterion) {
    let mut group = c.benchmark_group("reboot_cmd/from_str");

    // Each input is a heap-allocated String to match the real call site
    // (`argv[0]` is always a `String`).

    group.bench_function("init_bare", |b| {
        b.iter(|| RebootCMD::from("init"));
    });

    group.bench_function("poweroff_bare", |b| {
        b.iter(|| RebootCMD::from("poweroff"));
    });

    group.bench_function("reboot_bare", |b| {
        b.iter(|| RebootCMD::from("reboot"));
    });

    group.bench_function("unknown_bare", |b| {
        b.iter(|| RebootCMD::from("shutdown"));
    });

    // Full path variants — exercises the Path::file_name() strip.
    group.bench_function("init_full_path", |b| {
        b.iter(|| RebootCMD::from("/bin/init"));
    });

    group.bench_function("poweroff_full_path", |b| {
        b.iter(|| RebootCMD::from("/bin/poweroff"));
    });

    group.bench_function("reboot_full_path", |b| {
        b.iter(|| RebootCMD::from("/bin/reboot"));
    });

    group.bench_function("unknown_deep_path", |b| {
        b.iter(|| RebootCMD::from("/usr/local/sbin/some-unknown-tool"));
    });

    group.finish();
}

// ── RebootCMD ↔ RebootCommand conversions ────────────────────────────────────

fn bench_reboot_cmd_conversions(c: &mut Criterion) {
    use rustix::system::RebootCommand;

    let mut group = c.benchmark_group("reboot_cmd/conversions");

    group.bench_function("reboot_cmd_to_reboot_command", |b| {
        b.iter_batched(
            || RebootCMD::Reboot,
            |cmd| -> RebootCommand { cmd.into() },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("poweroff_cmd_to_reboot_command", |b| {
        b.iter_batched(
            || RebootCMD::PowerOff,
            |cmd| -> RebootCommand { cmd.into() },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("reboot_command_to_reboot_cmd", |b| {
        b.iter(|| RebootCMD::from(RebootCommand::Restart));
    });

    group.bench_function("poweroff_command_to_reboot_cmd", |b| {
        b.iter(|| RebootCMD::from(RebootCommand::PowerOff));
    });

    group.finish();
}

// ── Preboot construction ──────────────────────────────────────────────────────

fn bench_preboot_construction(c: &mut Criterion) {
    let mut group = c.benchmark_group("preboot/construction");

    // Preboot::new() calls is_dir() for each entry in VIRTUAL_FS — these are
    // live sysfs probes, so the benchmark captures real filesystem overhead.
    group.bench_function("new", |b| {
        b.iter(Preboot::new);
    });

    group.bench_function("default", |b| {
        b.iter(Preboot::default);
    });

    // Clone is cheap (Vec of static refs) but worth confirming.
    group.bench_function("clone", |b| {
        let preboot = Preboot::new();
        b.iter(|| preboot.clone());
    });

    group.finish();
}

// ── criterion wiring ──────────────────────────────────────────────────────────

criterion_group!(
    benches,
    bench_param_search,
    bench_param_search_scaling,
    bench_reboot_cmd_dispatch,
    bench_reboot_cmd_conversions,
    bench_preboot_construction,
);
criterion_main!(benches);
