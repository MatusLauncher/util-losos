//! Smoke tests for the `pakman` crate.
//!
//! Exercises:
//! * `PackageInstallation::new` / `default` — construction (reads `/proc/cmdline`).
//! * `PackageInstallation::add_to_queue`    — single-item and bulk queue growth.
//! * Dockerfile template rendering           — the `format!` that produces the build recipe.
//! * Tarball path derivation                 — `Path::new("/data/progs").join(…)`.
//! * `nerdctl` argument construction         — building a `Command` without spawning.
//! * `ProgRunner::new` / `default`           — zero-sized runner construction.
//! * Directory scan                          — `WalkDir`-based scan over a temp directory.
//! * String-match filter                     — the `.contains(prog)` predicate in isolation.

use std::{
    fs::File,
    hint::black_box,
    path::{Path, PathBuf},
    process::Command,
};

use pakman::{install::PackageInstallation, run::ProgRunner};
use walkdir::WalkDir;

const PKG_SHORT: &str = "jq";
const PKG_MEDIUM: &str = "curl";
const PKG_HYPHENATED: &str = "ripgrep";
const PKG_LONG: &str = "python3-with-packages";
const PKG_VERY_LONG: &str =
    "my-organisation-internal-tool-with-a-very-descriptive-and-verbose-package-name";

fn populate_progs_dir(dir: &Path, count: usize) -> Vec<PathBuf> {
    (0..count)
        .map(|i| {
            let p = dir.join(format!("pkg-{i:04}.tar"));
            File::create(&p).expect("failed to create bench tarball");
            p
        })
        .collect()
}

mod construction {
    use super::*;

    #[test]
    fn package_installation_new() {
        black_box(PackageInstallation::new());
    }

    #[test]
    fn package_installation_default() {
        black_box(PackageInstallation::default());
    }

    #[test]
    fn prog_runner_new() {
        black_box(ProgRunner::new());
    }

    #[test]
    fn prog_runner_default() {
        black_box(ProgRunner::default());
    }
}

mod add_to_queue_single {
    use super::*;

    #[test]
    fn short_name() {
        let mut pi = PackageInstallation::new();
        pi.add_to_queue(PKG_SHORT);
        black_box(pi);
    }

    #[test]
    fn medium_name() {
        let mut pi = PackageInstallation::new();
        pi.add_to_queue(PKG_MEDIUM);
        black_box(pi);
    }

    #[test]
    fn hyphenated_name() {
        let mut pi = PackageInstallation::new();
        pi.add_to_queue(PKG_HYPHENATED);
        black_box(pi);
    }

    #[test]
    fn long_name() {
        let mut pi = PackageInstallation::new();
        pi.add_to_queue(PKG_LONG);
        black_box(pi);
    }

    #[test]
    fn very_long_name() {
        let mut pi = PackageInstallation::new();
        pi.add_to_queue(PKG_VERY_LONG);
        black_box(pi);
    }
}

mod add_to_queue_bulk {
    use super::*;

    #[test]
    fn ten_packages() {
        let mut pi = PackageInstallation::new();
        for pkg in &["curl", "git", "jq", "ripgrep", "htop", "tree", "wget", "bat", "fd", "fzf"] {
            pi.add_to_queue(pkg);
        }
        black_box(pi);
    }

    #[test]
    fn fifty_packages() {
        let names: Vec<String> = (0..50).map(|i| format!("pkg-{i:02}")).collect();
        let mut pi = PackageInstallation::new();
        for name in &names {
            pi.add_to_queue(name);
        }
        black_box(pi);
    }
}

mod add_to_queue_scaling {
    use super::*;

    #[test]
    fn scaling() {
        for n in [1usize, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024] {
            let names: Vec<String> = (0..n).map(|i| format!("pkg-{i}")).collect();
            let mut pi = PackageInstallation::new();
            for name in &names {
                pi.add_to_queue(name);
            }
            black_box(pi);
        }
    }
}

mod dockerfile_template {
    use super::*;

    #[test]
    fn short_name() {
        black_box(format!(
            "FROM nixos/nix as base\nENTRYPOINT nix-shell -p {pkg} --run {pkg}",
            pkg = PKG_SHORT
        ));
    }

    #[test]
    fn medium_name() {
        black_box(format!(
            "FROM nixos/nix as base\nENTRYPOINT nix-shell -p {pkg} --run {pkg}",
            pkg = PKG_MEDIUM
        ));
    }

    #[test]
    fn hyphenated_name() {
        black_box(format!(
            "FROM nixos/nix as base\nENTRYPOINT nix-shell -p {pkg} --run {pkg}",
            pkg = PKG_HYPHENATED
        ));
    }

    #[test]
    fn long_name() {
        black_box(format!(
            "FROM nixos/nix as base\nENTRYPOINT nix-shell -p {pkg} --run {pkg}",
            pkg = PKG_LONG
        ));
    }

    #[test]
    fn very_long_name() {
        black_box(format!(
            "FROM nixos/nix as base\nENTRYPOINT nix-shell -p {pkg} --run {pkg}",
            pkg = PKG_VERY_LONG
        ));
    }
}

mod dockerfile_template_scaling {
    use super::*;

    #[test]
    fn scaling() {
        for len in [2usize, 4, 8, 16, 32, 64, 96, 128] {
            let name = "p".repeat(len);
            black_box(format!(
                "FROM nixos/nix as base\nENTRYPOINT nix-shell -p {name} --run {name}"
            ));
        }
    }
}

mod tarball_path {
    use super::*;

    #[test]
    fn short_name() {
        black_box(Path::new("/data/progs").join(format!("{}.tar", PKG_SHORT)));
    }

    #[test]
    fn medium_name() {
        black_box(Path::new("/data/progs").join(format!("{}.tar", PKG_MEDIUM)));
    }

    #[test]
    fn hyphenated_name() {
        black_box(Path::new("/data/progs").join(format!("{}.tar", PKG_HYPHENATED)));
    }

    #[test]
    fn long_name() {
        black_box(Path::new("/data/progs").join(format!("{}.tar", PKG_LONG)));
    }

    #[test]
    fn very_long_name() {
        black_box(Path::new("/data/progs").join(format!("{}.tar", PKG_VERY_LONG)));
    }
}

mod tarball_path_scaling {
    use super::*;

    #[test]
    fn scaling() {
        for len in [2usize, 4, 8, 16, 32, 64, 96, 128] {
            let name = "x".repeat(len);
            black_box(Path::new("/data/progs").join(format!("{name}.tar")));
        }
    }
}

mod nerdctl_command_build {
    use super::*;

    #[test]
    fn build_command() {
        let mut cmd = Command::new("/bin/nerdctl");
        cmd.arg("build")
            .arg(std::env::temp_dir().join(PKG_MEDIUM))
            .arg("-t")
            .arg(format!("local/{}", PKG_MEDIUM));
        black_box(cmd);
    }

    #[test]
    fn save_command() {
        let mut cmd = Command::new("/bin/nerdctl");
        cmd.arg("save")
            .arg(format!("local/{}", PKG_MEDIUM))
            .arg("-o")
            .arg(format!("/data/progs/{}.tar", PKG_MEDIUM));
        black_box(cmd);
    }

    #[test]
    fn load_command() {
        let mut cmd = Command::new("/bin/nerdctl");
        cmd.arg("load")
            .arg("-i")
            .arg(format!("/data/progs/{}.tar", PKG_MEDIUM));
        black_box(cmd);
    }

    #[test]
    fn run_command() {
        let mut cmd = Command::new("/bin/nerdctl");
        cmd.arg("run")
            .arg("-it")
            .arg(format!("localhost/local/{}", PKG_MEDIUM));
        black_box(cmd);
    }
}

mod nerdctl_command_scaling {
    use super::*;

    #[test]
    fn scaling() {
        for len in [2usize, 8, 16, 32, 64, 128] {
            let name = "p".repeat(len);
            let mut cmd = Command::new("/bin/nerdctl");
            cmd.arg("build")
                .arg(std::env::temp_dir().join(name.as_str()))
                .arg("-t")
                .arg(format!("local/{name}"))
                .arg("save")
                .arg(format!("local/{name}"))
                .arg("-o")
                .arg(format!("/data/progs/{name}.tar"));
            black_box(cmd);
        }
    }
}

mod directory_scan {
    use super::*;

    fn scan(dir: &str, needle: &str) -> Vec<String> {
        WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().display().to_string().contains(needle))
            .map(|e| e.path().display().to_string())
            .collect()
    }

    #[test]
    fn one_tarball() {
        let dir = tempfile::tempdir().unwrap();
        populate_progs_dir(dir.path(), 1);
        let dir_str = dir.path().to_str().unwrap();
        black_box(scan(dir_str, "pkg-0000"));
        black_box(scan(dir_str, "nonexistent"));
    }

    #[test]
    fn ten_tarballs() {
        let dir = tempfile::tempdir().unwrap();
        populate_progs_dir(dir.path(), 10);
        let dir_str = dir.path().to_str().unwrap();
        black_box(scan(dir_str, "pkg-0000"));
        black_box(scan(dir_str, "pkg-0009"));
        black_box(scan(dir_str, "nonexistent"));
    }

    #[test]
    fn hundred_tarballs() {
        let dir = tempfile::tempdir().unwrap();
        populate_progs_dir(dir.path(), 100);
        let dir_str = dir.path().to_str().unwrap();
        black_box(scan(dir_str, "pkg-0050"));
        black_box(scan(dir_str, "nonexistent"));
    }
}

mod directory_scan_scaling {
    use super::*;

    #[test]
    fn scaling() {
        for count in [1usize, 2, 4, 8, 16, 32, 64, 128, 256] {
            let dir = tempfile::tempdir().unwrap();
            populate_progs_dir(dir.path(), count);
            let dir_str = dir.path().to_str().unwrap().to_owned();
            let target = format!("pkg-{:04}", count - 1);

            black_box(
                WalkDir::new(&dir_str)
                    .into_iter()
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().display().to_string().contains(target.as_str()))
                    .map(|e| e.path().display().to_string())
                    .collect::<Vec<_>>(),
            );
        }
    }
}

mod contains_filter {
    use super::*;

    fn make_paths(n: usize) -> Vec<String> {
        (0..n)
            .map(|i| format!("/data/progs/pkg-{i:04}.tar"))
            .collect()
    }

    #[test]
    fn filter() {
        for count in [1usize, 8, 32, 128, 256] {
            let paths = make_paths(count);
            let target_hit = format!("pkg-{:04}", count / 2);
            black_box(
                paths
                    .iter()
                    .filter(|p| p.contains(target_hit.as_str()))
                    .collect::<Vec<_>>(),
            );
            black_box(
                paths
                    .iter()
                    .filter(|p| p.contains("nonexistent_pkg"))
                    .collect::<Vec<_>>(),
            );
        }
    }
}
