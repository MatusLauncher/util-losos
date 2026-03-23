# pakman — Package Manager

`pakman` is the package manager for the `util-mdl` initramfs OS. It installs programs on demand by building minimal NixOS-based container images with `nerdctl`, saving them as tarballs on a persistent data drive, and running them in isolated containers on request.

## Usage

```bash
pakman --install <pkg> [<pkg> ...]   # install one or more packages
pakman --remove  <pkg> [<pkg> ...]   # remove installed packages
pakman --run     <pkg>               # run an installed package
```

## How it works

### Installation (`--install`)

```
kernel cmdline → data_drive=<device>
    └─ PackageInstallation::start()
           ├─ read /proc/mounts — check if data_drive is already mounted
           ├─ mount <data_drive> → /data  (if not already mounted)
           ├─ mkdir -p /data/progs
           └─ per-package thread
                  ├─ write Dockerfile to $TMPDIR/<pkg>
                  │       FROM nixos/nix as base
                  │       ENTRYPOINT nix-shell -p <pkg> --run <pkg>
                  ├─ nerdctl build $TMPDIR/<pkg> -t local/<pkg>
                  └─ nerdctl save local/<pkg> -o /data/progs/<pkg>.tar
```

Each package is processed in a parallel thread via `std::thread::scope`. Installed tarballs survive reboots on the persistent data drive and are reloaded on demand without rebuilding.

### Removal (`--remove`)

Removes the saved tarball from `/data/progs/<pkg>.tar`. The package is no longer available to `--run` after removal.

### Running (`--run`)

```
ProgRunner::run(<pkg>)
    ├─ WalkDir /data/progs/ → find <pkg>.tar
    ├─ nerdctl load -i /data/progs/<pkg>.tar
    └─ nerdctl run -it localhost/local/<pkg>
```

The tarball is loaded back into the container runtime and run interactively.

## Configuration

`pakman` reads the kernel command line via `actman::cmdline::CmdLineOptions`. The only required key is:

| Key          | Description                                                         |
|--------------|---------------------------------------------------------------------|
| `data_drive` | Block device path for the persistent data partition (e.g. `/dev/sda2`). Required for `--install`. |

Set it in the kernel command line:

```
data_drive=/dev/sda2
```

## Requirements

- `nerdctl` must be available at `/bin/nerdctl` (provided by the initramfs image).
- The `data_drive` kernel command-line parameter must be set before running `--install`.
- `/data/progs/` is used as the persistent package store; the data drive must have sufficient space for image tarballs.

## CLI reference

```text
Usage: pakman [OPTIONS]

Options:
      --install <INSTALL>  Package(s) to install
      --remove <REMOVE>    Package(s) to remove
      --run <RUN>          Package to run
  -h, --help               Print help
  -V, --version            Print version
```

## Key source files

| File | Description |
|------|-------------|
| `crates/pakman/src/main.rs` | `clap` CLI entry point; dispatches to `PackageInstallation` or `ProgRunner` |
| `crates/pakman/src/install.rs` | `PackageInstallation` — mounts the data drive, builds and saves images in parallel threads |
| `crates/pakman/src/run.rs` | `ProgRunner` — loads a saved tarball and runs the container interactively |

## Architecture

```text
CLIface (clap)
    ├─ --install  →  PackageInstallation
    │                    ├─ CmdLineOptions  (reads data_drive from /proc/cmdline)
    │                    ├─ rustix::mount   (mounts data_drive → /data)
    │                    └─ thread::scope   (one thread per package)
    │                           └─ nerdctl build + nerdctl save → /data/progs/<pkg>.tar
    ├─ --remove   →  std::fs::remove_file(/data/progs/<pkg>.tar)
    └─ --run      →  ProgRunner
                         ├─ WalkDir /data/progs/  (find tarball by name)
                         ├─ nerdctl load -i <tarball>
                         └─ nerdctl run -it localhost/local/<pkg>
```

## Error handling and logging

All fallible operations return `miette::Result<()>`. Structured logging uses the `tracing` crate — key events (`Installing`, `Removing`, `Running`, mount checks) are emitted at the `info` level; already-mounted drives and missing arguments emit `warn`-level messages.