//! Benchmarks for the `pakman` crate.
//!
//! Covers:
//! * `PackageInstallation::new` / `default` — construction cost (reads `/proc/cmdline`).
//! * `PackageInstallation::add_to_queue` — single-item and bulk queue growth.
//! * Queue growth scaling — how `add_to_queue` cost changes from 1 to 1 024 items.
//! * Dockerfile template rendering — the `format!` call that produces the two-line
//!   build recipe; benchmarked at a range of package-name lengths.
//! * Tarball path derivation — `Path::new("/data/progs").join(format!("{pkg}.tar"))`,
//!   which is called once per package in the install hot path.
//! * `nerdctl` argument construction — building a `std::process::Command` with the
//!   full argument list for `nerdctl build` and `nerdctl save`, without spawning.
//! * `ProgRunner::new` / `default` — construction of the zero-sized runner type.
//! * Directory scan — a `WalkDir`-based scan over a temp directory at various
//!   tarball counts, mirroring the search performed by `ProgRunner::run`.
//! * Scan scaling — how scan time grows from 1 to 256 tarballs.
//! * String-match filter — the `.contains(prog)` predicate that selects tarballs
//!   by name, isolated from the I/O cost of the walk itself.

use std::{
    fs::File,
    path::{Path, PathBuf},
    process::Command,
};

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use pakman::{install::PackageInstallation, run::ProgRunner};
use walkdir::WalkDir;

// ── realistic fixtures ────────────────────────────────────────────────────────

/// Shortest realistic Nix package name.
const PKG_SHORT: &str = "jq";

/// Typical single-word package name.
const PKG_MEDIUM: &str = "curl";

/// Hyphenated package name common in Nixpkgs.
const PKG_HYPHENATED: &str = "ripgrep";

/// A longer, multi-component Nix attribute-path-style name.
const PKG_LONG: &str = "python3-with-packages";

/// A very long name that stresses the formatter's heap allocation.
const PKG_VERY_LONG: &str =
    "my-organisation-internal-tool-with-a-very-descriptive-and-verbose-package-name";

// ── PackageInstallation construction ─────────────────────────────────────────

fn bench_construction(c: &mut Criterion) {
    let mut group = c.benchmark_group("pakman/construction");

    // new() reads /proc/cmdline; the benchmark captures the full cold-start
    // cost including the file-system read and HashMap construction.
    group.bench_function("PackageInstallation::new", |b| {
        b.iter(PackageInstallation::new);
    });

    // default() delegates to new() so the cost should be identical.
    group.bench_function("PackageInstallation::default", |b| {
        b.iter(PackageInstallation::default);
    });

    // ProgRunner is a ZST — construction is purely a type-system operation.
    group.bench_function("ProgRunner::new", |b| {
        b.iter(ProgRunner::new);
    });

    group.bench_function("ProgRunner::default", |b| {
        b.iter(ProgRunner::default);
    });

    group.finish();
}

// ── add_to_queue — single call ────────────────────────────────────────────────

fn bench_add_to_queue_single(c: &mut Criterion) {
    let mut group = c.benchmark_group("pakman/add_to_queue/single");

    // Use iter_batched so each iteration gets a fresh PackageInstallation,
    // avoiding any amortisation from Vec pre-allocation across iterations.

    group.bench_function("short_name", |b| {
        b.iter_batched(
            PackageInstallation::new,
            |mut pi| {
                pi.add_to_queue(PKG_SHORT);
                pi
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("medium_name", |b| {
        b.iter_batched(
            PackageInstallation::new,
            |mut pi| {
                pi.add_to_queue(PKG_MEDIUM);
                pi
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("hyphenated_name", |b| {
        b.iter_batched(
            PackageInstallation::new,
            |mut pi| {
                pi.add_to_queue(PKG_HYPHENATED);
                pi
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("long_name", |b| {
        b.iter_batched(
            PackageInstallation::new,
            |mut pi| {
                pi.add_to_queue(PKG_LONG);
                pi
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("very_long_name", |b| {
        b.iter_batched(
            PackageInstallation::new,
            |mut pi| {
                pi.add_to_queue(PKG_VERY_LONG);
                pi
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ── add_to_queue — bulk sequential fill ──────────────────────────────────────

fn bench_add_to_queue_bulk(c: &mut Criterion) {
    let mut group = c.benchmark_group("pakman/add_to_queue/bulk");

    // Measure the cost of filling the queue with 10 packages in one go — the
    // typical upper bound for a single pakman --install invocation.
    group.bench_function("10_packages", |b| {
        b.iter_batched(
            PackageInstallation::new,
            |mut pi| {
                for pkg in &[
                    "curl", "git", "jq", "ripgrep", "htop", "tree", "wget", "bat", "fd", "fzf",
                ] {
                    pi.add_to_queue(pkg);
                }
                pi
            },
            BatchSize::SmallInput,
        );
    });

    // 50-package fill — stress-tests Vec reallocation.
    group.bench_function("50_packages", |b| {
        let names: Vec<String> = (0..50).map(|i| format!("pkg-{i:02}")).collect();
        b.iter_batched(
            PackageInstallation::new,
            |mut pi| {
                for name in &names {
                    pi.add_to_queue(name);
                }
                pi
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ── add_to_queue scaling ──────────────────────────────────────────────────────
//
// Shows how cumulative queue-fill time grows with the number of packages.

fn bench_add_to_queue_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("pakman/add_to_queue/scaling");

    for n in [1usize, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024] {
        let names: Vec<String> = (0..n).map(|i| format!("pkg-{i}")).collect();
        group.bench_with_input(BenchmarkId::from_parameter(n), &names, |b, names| {
            b.iter_batched(
                PackageInstallation::new,
                |mut pi| {
                    for name in names {
                        pi.add_to_queue(name);
                    }
                    pi
                },
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

// ── Dockerfile template rendering ─────────────────────────────────────────────
//
// The two-line Dockerfile is produced by a single `format!` inside the per-
// package install thread.  Benchmark the formatting cost at various name
// lengths because it runs once per package per `start()` call.

fn bench_dockerfile_template(c: &mut Criterion) {
    let mut group = c.benchmark_group("pakman/dockerfile_template");

    group.bench_function("short_name", |b| {
        b.iter(|| {
            format!(
                "FROM nixos/nix as base\nENTRYPOINT nix-shell -p {pkg} --run {pkg}",
                pkg = PKG_SHORT
            )
        });
    });

    group.bench_function("medium_name", |b| {
        b.iter(|| {
            format!(
                "FROM nixos/nix as base\nENTRYPOINT nix-shell -p {pkg} --run {pkg}",
                pkg = PKG_MEDIUM
            )
        });
    });

    group.bench_function("hyphenated_name", |b| {
        b.iter(|| {
            format!(
                "FROM nixos/nix as base\nENTRYPOINT nix-shell -p {pkg} --run {pkg}",
                pkg = PKG_HYPHENATED
            )
        });
    });

    group.bench_function("long_name", |b| {
        b.iter(|| {
            format!(
                "FROM nixos/nix as base\nENTRYPOINT nix-shell -p {pkg} --run {pkg}",
                pkg = PKG_LONG
            )
        });
    });

    group.bench_function("very_long_name", |b| {
        b.iter(|| {
            format!(
                "FROM nixos/nix as base\nENTRYPOINT nix-shell -p {pkg} --run {pkg}",
                pkg = PKG_VERY_LONG
            )
        });
    });

    group.finish();
}

// ── Dockerfile template scaling ───────────────────────────────────────────────
//
// Sweeps package-name length from 2 to 128 characters to confirm the
// format! cost scales linearly (or better) with name length.

fn bench_dockerfile_template_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("pakman/dockerfile_template/scaling");

    for len in [2usize, 4, 8, 16, 32, 64, 96, 128] {
        let name = "p".repeat(len);
        group.bench_with_input(BenchmarkId::from_parameter(len), &name, |b, name| {
            b.iter(|| {
                format!("FROM nixos/nix as base\nENTRYPOINT nix-shell -p {name} --run {name}")
            });
        });
    }

    group.finish();
}

// ── tarball path derivation ───────────────────────────────────────────────────
//
// Path::new("/data/progs").join(format!("{pkg}.tar")) is called once per
// package in the install hot path.  Both the format! and the join allocation
// are captured here.

fn bench_tarball_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("pakman/tarball_path");

    group.bench_function("short_name", |b| {
        b.iter(|| -> PathBuf { Path::new("/data/progs").join(format!("{}.tar", PKG_SHORT)) });
    });

    group.bench_function("medium_name", |b| {
        b.iter(|| -> PathBuf { Path::new("/data/progs").join(format!("{}.tar", PKG_MEDIUM)) });
    });

    group.bench_function("hyphenated_name", |b| {
        b.iter(|| -> PathBuf { Path::new("/data/progs").join(format!("{}.tar", PKG_HYPHENATED)) });
    });

    group.bench_function("long_name", |b| {
        b.iter(|| -> PathBuf { Path::new("/data/progs").join(format!("{}.tar", PKG_LONG)) });
    });

    group.bench_function("very_long_name", |b| {
        b.iter(|| -> PathBuf { Path::new("/data/progs").join(format!("{}.tar", PKG_VERY_LONG)) });
    });

    group.finish();
}

// ── tarball path scaling ──────────────────────────────────────────────────────

fn bench_tarball_path_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("pakman/tarball_path/scaling");

    for len in [2usize, 4, 8, 16, 32, 64, 96, 128] {
        let name = "x".repeat(len);
        group.bench_with_input(BenchmarkId::from_parameter(len), &name, |b, name| {
            b.iter(|| -> PathBuf { Path::new("/data/progs").join(format!("{name}.tar")) });
        });
    }

    group.finish();
}

// ── nerdctl argument construction ────────────────────────────────────────────
//
// Measures the cost of building a `Command` with all required arguments for
// each of the three nerdctl sub-commands used by pakman, *without* spawning a
// process.  Useful for confirming argument-string allocation overhead is
// negligible relative to process spawn time.

fn bench_nerdctl_command_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("pakman/nerdctl_command_build");

    // `nerdctl build <path> -t local/<pkg>`
    group.bench_function("build_command", |b| {
        b.iter(|| {
            let mut cmd = Command::new("/bin/nerdctl");
            cmd.arg("build")
                .arg(std::env::temp_dir().join(PKG_MEDIUM))
                .arg("-t")
                .arg(format!("local/{}", PKG_MEDIUM));
            cmd
        });
    });

    // `nerdctl save local/<pkg> -o /data/progs/<pkg>.tar`
    group.bench_function("save_command", |b| {
        b.iter(|| {
            let mut cmd = Command::new("/bin/nerdctl");
            cmd.arg("save")
                .arg(format!("local/{}", PKG_MEDIUM))
                .arg("-o")
                .arg(format!("/data/progs/{}.tar", PKG_MEDIUM));
            cmd
        });
    });

    // `nerdctl load -i /data/progs/<pkg>.tar`
    group.bench_function("load_command", |b| {
        b.iter(|| {
            let mut cmd = Command::new("/bin/nerdctl");
            cmd.arg("load")
                .arg("-i")
                .arg(format!("/data/progs/{}.tar", PKG_MEDIUM));
            cmd
        });
    });

    // `nerdctl run -it localhost/local/<pkg>`
    group.bench_function("run_command", |b| {
        b.iter(|| {
            let mut cmd = Command::new("/bin/nerdctl");
            cmd.arg("run")
                .arg("-it")
                .arg(format!("localhost/local/{}", PKG_MEDIUM));
            cmd
        });
    });

    group.finish();
}

// ── nerdctl argument construction scaling ─────────────────────────────────────
//
// Confirms that Command argument allocation scales with package name length.

fn bench_nerdctl_command_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("pakman/nerdctl_command_build/scaling");

    for len in [2usize, 8, 16, 32, 64, 128] {
        let name = "p".repeat(len);
        group.bench_with_input(BenchmarkId::from_parameter(len), &name, |b, name| {
            b.iter(|| {
                let mut cmd = Command::new("/bin/nerdctl");
                cmd.arg("build")
                    .arg(std::env::temp_dir().join(name.as_str()))
                    .arg("-t")
                    .arg(format!("local/{name}"))
                    .arg("save")
                    .arg(format!("local/{name}"))
                    .arg("-o")
                    .arg(format!("/data/progs/{name}.tar"));
                cmd
            });
        });
    }

    group.finish();
}

// ── directory scan (find_tarball behaviour) ───────────────────────────────────
//
// `ProgRunner::run` walks `/data/progs` with WalkDir and filters entries by a
// `.contains(prog)` substring test.  `find_tarball` is private to the crate,
// so the benchmark re-implements the identical logic using the same `walkdir`
// crate that pakman depends on, against a controlled temporary directory.
//
// The temporary directories are set up once in the benchmark setup closure
// so the I/O cost of `File::create` is excluded from the measured path.

/// Creates `count` uniquely-named `.tar` files inside `dir` and returns
/// their paths.  Uses a zero-allocation naming scheme to keep setup fast.
fn populate_progs_dir(dir: &Path, count: usize) -> Vec<PathBuf> {
    (0..count)
        .map(|i| {
            let p = dir.join(format!("pkg-{i:04}.tar"));
            File::create(&p).expect("failed to create bench tarball");
            p
        })
        .collect()
}

fn bench_directory_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("pakman/directory_scan");

    // ── 1 tarball ──────────────────────────────────────────────────────────────
    let dir_1 = tempfile::tempdir().expect("failed to create tempdir");
    populate_progs_dir(dir_1.path(), 1);
    let dir_1_str = dir_1.path().to_str().unwrap().to_owned();

    group.bench_function("1_tarball_hit", |b| {
        b.iter(|| {
            WalkDir::new(&dir_1_str)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().display().to_string().contains("pkg-0000"))
                .map(|e| e.path().display().to_string())
                .collect::<Vec<_>>()
        });
    });

    group.bench_function("1_tarball_miss", |b| {
        b.iter(|| {
            WalkDir::new(&dir_1_str)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().display().to_string().contains("nonexistent"))
                .map(|e| e.path().display().to_string())
                .collect::<Vec<_>>()
        });
    });

    // ── 10 tarballs ────────────────────────────────────────────────────────────
    let dir_10 = tempfile::tempdir().expect("failed to create tempdir");
    populate_progs_dir(dir_10.path(), 10);
    let dir_10_str = dir_10.path().to_str().unwrap().to_owned();

    group.bench_function("10_tarballs_hit_first", |b| {
        b.iter(|| {
            WalkDir::new(&dir_10_str)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().display().to_string().contains("pkg-0000"))
                .map(|e| e.path().display().to_string())
                .collect::<Vec<_>>()
        });
    });

    group.bench_function("10_tarballs_hit_last", |b| {
        b.iter(|| {
            WalkDir::new(&dir_10_str)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().display().to_string().contains("pkg-0009"))
                .map(|e| e.path().display().to_string())
                .collect::<Vec<_>>()
        });
    });

    group.bench_function("10_tarballs_miss", |b| {
        b.iter(|| {
            WalkDir::new(&dir_10_str)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().display().to_string().contains("nonexistent"))
                .map(|e| e.path().display().to_string())
                .collect::<Vec<_>>()
        });
    });

    // ── 100 tarballs ───────────────────────────────────────────────────────────
    let dir_100 = tempfile::tempdir().expect("failed to create tempdir");
    populate_progs_dir(dir_100.path(), 100);
    let dir_100_str = dir_100.path().to_str().unwrap().to_owned();

    group.bench_function("100_tarballs_hit", |b| {
        b.iter(|| {
            WalkDir::new(&dir_100_str)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().display().to_string().contains("pkg-0050"))
                .map(|e| e.path().display().to_string())
                .collect::<Vec<_>>()
        });
    });

    group.bench_function("100_tarballs_miss", |b| {
        b.iter(|| {
            WalkDir::new(&dir_100_str)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().display().to_string().contains("nonexistent"))
                .map(|e| e.path().display().to_string())
                .collect::<Vec<_>>()
        });
    });

    group.finish();
}

// ── directory scan scaling ────────────────────────────────────────────────────
//
// Shows how WalkDir + contains scan time grows with the number of tarballs
// in the store.  The target (matching) entry is always the last one written so
// the scan must read every preceding entry before finding it.

fn bench_directory_scan_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("pakman/directory_scan/scaling");

    for count in [1usize, 2, 4, 8, 16, 32, 64, 128, 256] {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        populate_progs_dir(dir.path(), count);
        let dir_str = dir.path().to_str().unwrap().to_owned();
        // The target is always the last entry — worst-case scan depth.
        let target = format!("pkg-{:04}", count - 1);

        group.bench_with_input(
            BenchmarkId::new("hit_last", count),
            &(dir_str.clone(), target.clone()),
            |b, (dir, target): &(String, String)| {
                b.iter(|| {
                    WalkDir::new(dir)
                        .into_iter()
                        .filter_map(|e: walkdir::Result<walkdir::DirEntry>| e.ok())
                        .filter(|e: &walkdir::DirEntry| {
                            e.path().display().to_string().contains(target.as_str())
                        })
                        .map(|e: walkdir::DirEntry| e.path().display().to_string())
                        .collect::<Vec<_>>()
                });
            },
        );

        group.bench_with_input(BenchmarkId::new("miss", count), &dir_str, |b, dir| {
            b.iter(|| {
                WalkDir::new(dir)
                    .into_iter()
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().display().to_string().contains("nonexistent_pkg"))
                    .map(|e| e.path().display().to_string())
                    .collect::<Vec<_>>()
            });
        });
    }

    group.finish();
}

// ── string-match filter in isolation ─────────────────────────────────────────
//
// Isolates the `.contains(prog)` predicate from the I/O cost of WalkDir by
// running it against a pre-built Vec of path strings.  This separates
// "how expensive is the string search?" from "how expensive is the readdir?".

fn bench_contains_filter(c: &mut Criterion) {
    let mut group = c.benchmark_group("pakman/contains_filter");

    // Build a Vec of display-path strings that mimics what WalkDir would
    // produce for a store with N entries.
    let make_paths = |n: usize| -> Vec<String> {
        (0..n)
            .map(|i| format!("/data/progs/pkg-{i:04}.tar"))
            .collect()
    };

    for count in [1usize, 8, 32, 128, 256] {
        let paths = make_paths(count);
        let target_hit = format!("pkg-{:04}", count / 2); // mid-point hit
        let target_miss = "nonexistent_pkg".to_owned();

        group.bench_with_input(
            BenchmarkId::new("hit", count),
            &(paths.clone(), target_hit),
            |b, (paths, target)| {
                b.iter(|| {
                    paths
                        .iter()
                        .filter(|p| p.contains(target.as_str()))
                        .collect::<Vec<_>>()
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("miss", count),
            &(paths.clone(), target_miss),
            |b, (paths, target)| {
                b.iter(|| {
                    paths
                        .iter()
                        .filter(|p| p.contains(target.as_str()))
                        .collect::<Vec<_>>()
                });
            },
        );
    }

    group.finish();
}

// ── criterion wiring ──────────────────────────────────────────────────────────

criterion_group!(
    benches,
    bench_construction,
    bench_add_to_queue_single,
    bench_add_to_queue_bulk,
    bench_add_to_queue_scaling,
    bench_dockerfile_template,
    bench_dockerfile_template_scaling,
    bench_tarball_path,
    bench_tarball_path_scaling,
    bench_nerdctl_command_build,
    bench_nerdctl_command_scaling,
    bench_directory_scan,
    bench_directory_scan_scaling,
    bench_contains_filter,
);
criterion_main!(benches);
