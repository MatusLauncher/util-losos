# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`util-mdl` is a minimal, container-native OS init system and update manager packaged as a bootable initramfs. Each component lives in its own Git repository under the `losos-linux` namespace and is linked here as a git submodule:

| Submodule path | Repository | Description |
|---|---|---|
| `crates/actman` | [losos-linux/actman](https://gitlab.com/losos-linux/actman) | Init system (PID 1) |
| `crates/updman` | [losos-linux/updman](https://gitlab.com/losos-linux/updman) | Update manager |
| `crates/pakman` | [losos-linux/pakman](https://gitlab.com/losos-linux/pakman) | Package manager |
| `crates/cluman` | [losos-linux/cluman](https://gitlab.com/losos-linux/cluman) | Cluster/container manager |
| `crates/isoman` | [losos-linux/isoman](https://gitlab.com/losos-linux/isoman) | ISO/initramfs builder |
| `crates/dhcman` | [losos-linux/dhcman](https://gitlab.com/losos-linux/dhcman) | DHCP client |
| `crates/testman` | [losos-linux/testman](https://gitlab.com/losos-linux/testman) | Integration test harness |
| `crates/user` | [losos-linux/userman](https://gitlab.com/losos-linux/userman) | User management (userman + perman) |
| `crates/gpuman` | [losos-linux/gpuman](https://gitlab.com/losos-linux/gpuman) | GPU/NPU accelerator manager |
| `book` | [losos-linux/docs](https://gitlab.com/losos-linux/docs) | Documentation |

The `crates/bench` directory is the only in-tree crate — it contains integration benchmarks that depend on all other crates via git dependencies.

## Getting Started

```bash
# Clone with all submodules
git clone --recurse-submodules https://gitlab.com/losos-linux/util-mdl.git

# Or initialize submodules after cloning
git submodule update --init --recursive
```

## Build Commands

```bash
# Build bench (the only workspace member)
cargo build

# Build a specific submodule crate
cargo build --manifest-path crates/actman/Cargo.toml

# Release build
cargo build --release --manifest-path crates/actman/Cargo.toml

# Static MUSL target (used in container/initramfs)
cargo build --release --target x86_64-unknown-linux-musl --manifest-path crates/actman/Cargo.toml

# Build the full initramfs container image
podman build --no-cache -t util-mdl-build .

# Launch in QEMU (requires pre-built initramfs + kernel) — via Justfile
just run

# Build initramfs then launch — via Justfile
just build-run
```

The Justfile is the primary interface for building, launching, and testing. It respects env vars: `KERNEL` (path to vmlinuz), `MEMORY` (default `2G`), `CPUS` (default `2`), `KVM` (default `1`), `DISK` (host disk image to attach).

## Lint and Test

```bash
cargo clippy
cargo fmt --check
```

Integration tests run via `testman` — boots the initramfs in QEMU and asserts expected log output:

```bash
# Run tests against a pre-built initramfs — via Justfile
just test

# Build initramfs first, then run tests
just build-test

# Disable KVM for CI environments
just KVM=0 test
```

`testman` respects env vars: `KERNEL`, `INITRAMFS`, `ISO` (default `os.iso`), `MEMORY` (default `2G`), `CPUS` (default `2`), `KVM` (default `1`), `TEST_MODE` (`qemu` (default) or `container` to run QEMU inside Docker via testcontainers).

## Git Workflow — Auto Commit and Push

**Always commit and push changes automatically after making modifications.** Follow these rules:

### For Submodule Changes (crates/*, book)

When you modify files inside a submodule (e.g., `crates/cluman/src/*.rs`):

```bash
# 1. Navigate to the submodule
cd crates/<submodule-name>

# 2. Stage all changes
git add -A

# 3. Commit with a descriptive message
git commit -m "<type>: <description>

<detailed explanation of changes>"

# 4. Push to remote
git push

# 5. Return to root and update submodule reference
cd ../..
git add crates/<submodule-name>
git commit -m "chore: update <submodule-name> submodule"
git push
```

### For Root Repository Changes

When you modify files in the root repository (e.g., `Cargo.lock`, `README.md`):

```bash
# 1. Stage changes
git add -A

# 2. Commit with descriptive message
git commit -m "<type>: <description>

<detailed explanation>"

# 3. Push to remote
git push
```

### Commit Message Conventions

Use conventional commit format:

| Type | When to Use |
|------|-------------|
| `feat` | New feature or capability |
| `fix` | Bug fix |
| `refactor` | Code restructuring without behavior change |
| `docs` | Documentation only |
| `test` | Adding or modifying tests |
| `chore` | Maintenance tasks, dependency updates, submodule bumps |

### Important Notes

- **Always commit submodules first**, then update the parent repository
- **Never push without committing** — ensure all changes are committed
- **Use `git add -A`** to stage all changes including deletions
- **Check `git status`** before committing to verify what will be included
- **Run tests** (`cargo nextest run`) before committing code changes
- **Run linting** (`cargo clippy`, `cargo fmt --check`) before committing Rust code

### Pre-commit Checklist

Before pushing, verify:
1. ✅ Code compiles (`cargo build` or `cargo build --manifest-path crates/<name>/Cargo.toml`)
2. ✅ Tests pass (`cargo nextest run`)
3. ✅ No clippy warnings (`cargo clippy`)
4. ✅ Code is formatted (`cargo fmt`)
5. ✅ Documentation updated if API changed (update `book/src/*.md`)

## Documentation & Book

The project documentation is located in the `book/` submodule. **Always update the relevant markdown files in `book/src/` whenever you change features, APIs, or architectural components.**

- Use `mdbook build` to verify changes.
- Ensure new files are added to `book/src/SUMMARY.md`.

## Kernel Configuration

The kernel is configured in the root `Justfile` under the `kernel` recipe using `./scripts/config`.

- **To add a feature**: Add `-e CONFIG_FEATURE_NAME` to the `./scripts/config` call.
- **To remove a feature**: Add `-d CONFIG_FEATURE_NAME` to the `./scripts/config` call.
- Always run `just kernel` after modifying the configuration to verify it still builds.

## Architecture

### Submodule Structure

Each crate is an independent Git repository. Inter-crate dependencies use git URLs:

```
actman (core library, no deps)
  ├─→ updman
  ├─→ pakman
  ├─→ cluman
  │   └─→ isoman
  └─→ userman (also depends on perman via path within same repo)

gpuman (depends on actman)
dhcman (standalone)
testman (standalone)
bench (in-tree, depends on all via git deps)
```

### actman — Init System

Boot path: kernel executes `/bin/init` (actman binary) → determines mode from `argv[0]`:

- **Init mode**: calls `Preboot::mount()` to set up filesystems, then walks `/etc/init/start/` and spawns each script in order.
- **PowerOff/Reboot mode**: walks `/etc/init/stop/` running shutdown scripts, then calls `rustix::system::reboot()` with the appropriate `RebootCommand`.

Symlinks created in the container image: `/bin/poweroff → /bin/init`, `/bin/reboot → /bin/init`.

### updman — Update Manager

Reads `/etc/update.json` (`base_url`, `image_tag`, `hash`) → runs `nerdctl save` to pull/export the container image → extracts the nested `os.initramfs.tar.gz` → mounts the `BOOT` partition and replaces the initramfs file.

### pakman — Package Manager

CLI tool for installing, removing, and running programs inside the initramfs environment. Uses NixOS container images built with `nerdctl` and stores the resulting tarballs on a persistent data drive.

### gpuman — GPU/NPU Accelerator Manager

Detects GPUs and neural accelerators at boot via sysfs (`/sys/class/drm/card*` for GPUs, `/sys/class/accel/accel*` for NPUs) and launches vendor-specific containers with the full driver/runtime stack. Supports NVIDIA (CUDA), AMD (ROCm), and Intel (oneAPI). Container images are configurable via kernel command-line parameters (`gpu_nvidia_image=`, `gpu_amd_image=`, `gpu_intel_image=`). Uses symlink polymorphism: `gpuman` → Daemon mode (detect + launch), `gpuctl` → CLI query tool. The initramfs includes GPU firmware from `linux-firmware` so the kernel can initialise the hardware.

### Error Handling and Logging

All fallible operations return `miette::Result<()>` using `IntoDiagnostic` trait conversions. Structured logging via the `tracing` crate (`info!()` macro throughout).

### Container Build (Containerfile)

Multi-stage OCI build:
1. **stage0**: Alpine + busybox-static + nerdctl download + GPU/NPU firmware (linux-firmware)
2. **util**: Rust + musl — compiles crates for `x86_64-unknown-linux-musl`
3. **stage1**: Assembles final filesystem hierarchy, creates cpio archive → `os.initramfs.tar.gz`

The final artifact (`os.initramfs.tar.gz`) is what gets deployed to the BOOT partition and loaded by the kernel as an initramfs.

## Profiling — Agent Workflow

Non-prod ISOs built with `--profile profiling` embed Valgrind and stream
profiling output back to the host over the QEMU serial port.

### Build

```bash
just build-profiling          # or: isoman --build --profile profiling
```

Add a `build-profiling` recipe to the Justfile if it doesn't exist yet:
```just
build-profiling:
    cargo run --manifest-path crates/isoman/Cargo.toml -- \
        --build --profile profiling --mode client
```

### Capture (Rust — testman)

```rust
use testman::{HarnessConfig, TestHarness};
use std::time::Duration;

let cfg = HarnessConfig::from_env();
let mut h = TestHarness::start(cfg)?;
h.wait_for("login:", Duration::from_secs(120))?;

h.send("callgrind /bin/actman")?;
if let Some(cap) = h.collect_profile(Duration::from_secs(300))? {
    std::fs::write(&cap.filename, &cap.content)?;
    // Feed cap.content to an AI agent for hotspot analysis
}
h.shutdown();
```

### Serial protocol (tool-agnostic)

```
<<<PROFILE_BEGIN:tool:filename>>>
[verbatim KCachegrind-format text — plain ASCII, no encoding]
<<<PROFILE_END>>>
```

The `content` field is standard KCachegrind format.  Pass it verbatim to an
LLM with a prompt such as:
> "Annotate the hottest functions in this callgrind output and suggest
> specific Rust optimisations for the top 5 by inclusive instruction count."
