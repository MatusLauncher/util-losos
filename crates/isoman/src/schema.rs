//! Containerfile template and mode-injection for the initramfs image build.
//!
//! This module owns the multi-stage Containerfile strings that `isoman` writes
//! to disk before invoking `podman build`.  The template is parameterised by
//! one value — the `cluman` operating mode — which is baked in via
//! [`ContMode::return_final_contf`].
//!
//! # Build stages
//!
//! | Stage | Base image | Purpose |
//! |-------|-----------|---------|
//! | `stage0` | `alpine:latest` | Downloads `busybox-static` and the latest `nerdctl` full bundle; creates the target filesystem tree under `out/`. |
//! | `util`   | `rust:alpine`   | Compiles all workspace binaries for `x86_64-unknown-linux-musl`.  `perman` is compiled as an `rlib` and linked statically into `userman` — no shared library is produced. |
//! | `stage1` | `alpine:latest` | Assembles the final filesystem, writes init scripts, and packs everything into a newc cpio archive compressed as `os.tar.gz`. |
//! | *(final)* | `scratch`      | Exports `os.tar.gz` as `os.initramfs.tar.gz`, the sole artifact consumed by the ISO assembly step. |

/// Zig release used as the musl C compiler / linker inside the build container.
const ZIG_VERSION: &str = "0.13.0";

/// Scaffolding stage: sets up the base filesystem tree and downloads nerdctl.
const STAGE0: &str = r#"# check=skip=FromAsCasing
# scaffolding
FROM alpine:latest as stage0
RUN apk add busybox-static
RUN mkdir -p \
    out/dev \
    out/run \
    out/sys \
    out/proc \
    out/tmp \
    out/home \
    out/bin \
    out/lib \
    out/etc/init/start \
    out/etc/init/stop
RUN cp /bin/busybox.static out/bin/busybox
RUN apk add curl tar
RUN curl -LO $(curl -s https://api.github.com/repos/containerd/nerdctl/releases/latest | grep full | grep browser_download_url | head -n1 | awk '{print $2}' | cut -d '"' -f2)
RUN tar -xpf nerdctl*.tar.gz -C out/ \
    bin/nerdctl \
    bin/containerd \
    bin/containerd-shim-runc-v2 \
    bin/buildkitd \
    bin/runc \
    libexec/cni/
RUN cd out/bin && for applet in $(/out/bin/busybox --list); do ln -sf busybox "$applet"; done
"#;

/// Build stage: compiles all workspace binaries for `x86_64-unknown-linux-musl`.
///
/// `perman` is compiled as a regular `rlib` and linked statically into
/// `userman`; no separate shared-library artifact is produced.
const STAGE_UTIL: &str = r#"
# init + package manager
FROM rust:alpine as util
RUN apk add pkgconfig eudev-dev linux-headers
COPY . /mdl
RUN cd /mdl \
    && cargo build --release --target x86_64-unknown-linux-musl --workspace --exclude isoman \
    && cp target/x86_64-unknown-linux-musl/release/actman /actman \
    && cp target/x86_64-unknown-linux-musl/release/updman /updman \
    && cp target/x86_64-unknown-linux-musl/release/dhcman /dhcman \
    && cp target/x86_64-unknown-linux-musl/release/cluman /cluman \
    && cp target/x86_64-unknown-linux-musl/release/userman /userman \
    && rm -rf target /root/.cargo/registry
"#;

/// Assembly stage: builds the final filesystem and packs it into a cpio archive.
///
/// Contains the bare `ARG MODE` declaration; [`ContMode::return_final_contf`]
/// replaces it with `ARG MODE=<value>` before the file is written to disk.
const STAGE1: &str = r#"
# packaging
FROM alpine:latest as stage1
ARG MODE
COPY --from=stage0 out out
COPY --from=util /actman out/bin/init
COPY --from=util /updman out/bin/updman
COPY --from=util /dhcman out/bin/dhcman
COPY --from=util /cluman out/bin/cluman
COPY --from=util /userman out/bin/userman
RUN cd out && ln -sf bin sbin
RUN printf '#!/bin/sh\nip link set lo up && ip addr add 127.0.0.1/8 dev lo\n' > out/etc/init/start/00-loopback \
    && chmod +x out/etc/init/start/00-loopback
RUN cd out && ln -sf /bin/dhcman etc/init/start/00-eth0
RUN cd out && ln -sf /bin/userman etc/init/start/login
RUN cd out && ln -sf /bin/userman etc/init/start/usersvc-local
RUN cd out && ln -sf /bin/buildkitd etc/init/start/buildkitd
RUN cd out && ln -sf /bin/containerd etc/init/start/containerd
RUN cd out && ln -sf nerdctl bin/docker
RUN cd out && ln -sf nerdctl bin/podman
RUN cd out && ln -sf init bin/poweroff
RUN cd out && ln -sf init bin/reboot
RUN cd out && ln -sf bin/init init
# cluman mode — the binary dispatches on argv[0], so symlink it to the mode name.
# client/server are boot-time daemons started by init; controller is a one-shot
# CLI tool and is only installed as a named symlink without an init entry.
RUN cd out && ln -sf /bin/cluman etc/init/start/$MODE
RUN printf 'export PS1="$USER:$PWD$ "\n' > out/etc/profile
RUN apk add fakeroot
RUN fakeroot sh -c 'mknod out/dev/console c 5 1 && cd out && find . | cpio -o -H newc | gzip > ../os.tar.gz'
"#;

/// Export stage: copies the initramfs archive out of the build as the sole artifact.
const STAGE_FINAL: &str = r#"
# final
FROM scratch
COPY --from=stage1 os.tar.gz os.initramfs.tar.gz
"#;

/// Renders the Containerfile template with a specific `cluman` mode baked in.
///
/// Call [`set_mode`](ContMode::set_mode) to choose the operating mode, then
/// [`return_final_contf`](ContMode::return_final_contf) to obtain the
/// ready-to-write Containerfile string.  The default mode is whatever
/// [`cluman::schemas::Mode`] implements as its [`Default`].
///
/// # Example
///
/// ```rust,ignore
/// let mut cm = ContMode::new();
/// cm.set_mode(Mode::Server);
/// let containerfile = cm.return_final_contf();
/// std::fs::write("Containerfile.generated", &containerfile)?;
/// ```
#[derive(Default)]
pub struct ContMode {
    mode: cluman::schemas::Mode,
}

impl ContMode {
    /// Creates a new `ContMode` with the default [`cluman::schemas::Mode`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the `cluman` operating mode to embed in the generated Containerfile.
    ///
    /// Returns `&Self` so calls can be chained with
    /// [`return_final_contf`](ContMode::return_final_contf).
    ///
    /// Accepted modes (defined by `cluman`):
    /// - `client` — boot-time daemon, started by init on every boot.
    /// - `server` — boot-time daemon, started by init on every boot.
    /// - `controller` — one-shot CLI tool; installed as a named symlink only.
    pub fn set_mode(&mut self, mode: cluman::schemas::Mode) -> &Self {
        self.mode = mode;
        self
    }

    /// Returns the fully rendered Containerfile as an owned [`String`].
    ///
    /// - Replaces the bare `ARG MODE` declaration in [`STAGE1`] with
    ///   `ARG MODE=<mode>`, so `podman build` does not require an explicit
    ///   `--build-arg MODE=…` flag.
    /// - Substitutes `ZIG_VERSION` placeholders in [`STAGE_UTIL`] with the
    ///   [`ZIG_VERSION`] constant.
    pub fn return_final_contf(&self) -> String {
        let util = STAGE_UTIL.replace("ZIG_VERSION", ZIG_VERSION);
        let stage1 = STAGE1.replace("ARG MODE", &format!("ARG MODE={}", self.mode));
        format!("{STAGE0}{util}{stage1}{STAGE_FINAL}")
    }
}
