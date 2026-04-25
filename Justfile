# util-mdl — build, launch, and test the initramfs OS.
#
# Recipes:
#   just build               Build OS disk image (isoman built in Alpine, full container assembly)
#   just container-build     Build OS disk image with Rust compiled fully inside nerdctl
#   just build-config        Build from a JSON config file (ISOMAN_CONFIG)
#   just build-gsi           Build a GSI (Fastboot + Odin)
#   just build-gsi-fastboot  Build a Fastboot-only GSI boot.img
#   just build-secure-boot   Build with Secure Boot signing
#   just build-prod          Build production-hardened image (loglevel=0 + mitigations)
#   just build-prod-live     Build production live image (hardened + container-ready for preflight)
#   just run                 Launch in QEMU (UEFI via OVMF)
#   just test                Run testman integration tests
#   just build-run           Build then launch
#   just build-test          Build then test
#   just dev                 Pull all submodules and build the workspace
#   just kernel-profiles     Collect AutoFDO samples; next 'just kernel' applies them
#   just setup-sbctl         Generate Secure Boot key hierarchy (PK/KEK/db) via sbctl in an ephemeral Alpine container
# ── Configurable variables (override via env or 'just var=value recipe') ──────

kernel := env("KERNEL", "vmlinuz")
memory := env("MEMORY", "2G")
cpus := env("CPUS", "4")
kvm := env("KVM", "1")
disk := env("DISK", "")
output := env("OUTPUT", "os-client.iso")
ovmf_code := env("OVMF_CODE", "/usr/share/edk2/x64/OVMF_CODE.4m.fd")
ovmf_vars := env("OVMF_VARS", "/usr/share/edk2/x64/OVMF_VARS.4m.fd")
isoman_config := env("ISOMAN_CONFIG", "")
kernel_tag := `git ls-remote --tags --refs https://github.com/torvalds/linux 'v*' | awk '{print $2}' | sed 's#refs/tags/##' | sort -V | cut -d '-' -f1 | tail -n1`
threads := `nproc`
pwd := `pwd`
build_cache := env("BUILD_CACHE", ".build-cache")
# Persistent directory for the nerdctl full bundle (survives host reboots).
nerdctl_bundle := pwd + "/" + build_cache + "/nerdctl-bin"
# nerdctl binary: prefer system install, fall back to persistent bundle copy.
[private]
_nerdctl_system := `command -v nerdctl 2>/dev/null || true`
nerdctl_bin := if _nerdctl_system != "" { _nerdctl_system } else { nerdctl_bundle + "/bin/nerdctl" }
# Pre-built isoman binary (built inside Alpine by _build-isoman).
isoman_bin := env("ISOMAN_BIN", ".build-cache/isoman")
# ── Private helpers ───────────────────────────────────────────────────────────

# Ensure cargo-nextest is installed
_ensure-nextest:
    #!/bin/sh
    if ! command -v cargo-nextest >/dev/null 2>&1; then
        echo "==> cargo-nextest not found — installing..."
        cargo install cargo-nextest --locked
    fi

# Load dm-integrity and ensure root access for cryptsetup loop-device operations.
_dm-integrity:
    #!/bin/sh
    if ! lsmod | grep -q "^dm_integrity" && ! grep -q "^dm_integrity" /proc/modules; then
        sudo modprobe dm-integrity || echo "WARNING: dm-integrity unavailable (restricted environment) — continuing."
    fi

# Download the nerdctl *full* bundle to {{ nerdctl_bundle }}/.
# The full bundle (bin/ + lib/cni/) includes containerd, runc, buildkitd,
# containerd-rootless-setuptool.sh, and the CNI plugins — everything needed
# to bootstrap rootless containerd without any host container runtime installed.
# Skipped when the setup script is already present (bundle already extracted).
_ensure-nerdctl:
    #!/bin/sh
    set -eu
    SETUP={{ nerdctl_bundle }}/bin/containerd-rootless-setuptool.sh
    [ -f "$SETUP" ] && exit 0
    echo "==> Downloading nerdctl full bundle to {{ nerdctl_bundle }}/ ..."
    mkdir -p {{ nerdctl_bundle }}
    API=$(curl -fsSL https://api.github.com/repos/containerd/nerdctl/releases/latest)
    URL=$(printf '%s\n' "$API" \
        | grep 'browser_download_url' \
        | grep 'nerdctl-full.*linux-amd64\.tar\.gz' \
        | head -1 \
        | cut -d '"' -f4)
    if [ -z "$URL" ]; then
        echo "ERROR: could not find nerdctl full bundle URL." >&2
        printf '%s\n' "$API" | head -10 >&2
        exit 1
    fi
    echo "==> Fetching $URL"
    curl -fsSL "$URL" | tar -xz -C {{ nerdctl_bundle }}
    echo "==> nerdctl full bundle extracted to {{ nerdctl_bundle }}/"

# Ensure rootless containerd is running.
# On first run: patches containerd-rootless-setuptool.sh to recognise the
# LosOS/actman init system, runs it, then starts containerd if needed.
_ensure-containerd-rootless: _ensure-nerdctl
    #!/bin/sh
    set -eu
    export PATH="{{ nerdctl_bundle }}/bin:${PATH}"
    SOCK="/run/user/$(id -u)/containerd/containerd.sock"
    ROOTLESSKIT_STATE="/run/user/$(id -u)/containerd-rootless"
    SETUP="{{ nerdctl_bundle }}/bin/containerd-rootless-setuptool.sh"
    ROOTLESS="{{ nerdctl_bundle }}/bin/containerd-rootless.sh"
    [ -f "$SETUP" ] || { echo "ERROR: $SETUP missing — delete {{ nerdctl_bundle }}/ to redownload" >&2; exit 1; }

    # Verify the socket is live, not just a stale file left by a dead process.
    if [ -S "$SOCK" ]; then
        if "{{ nerdctl_bin }}" --address "$SOCK" info >/dev/null 2>&1; then
            exit 0
        fi
        echo "==> Stale containerd socket detected — cleaning up and restarting..."
        rm -f "$SOCK" "${SOCK}.ttrpc"
        # The RootlessKit lock must also be removed; without this a new
        # containerd-rootless.sh launch fails with "lock already held".
        rm -f "$ROOTLESSKIT_STATE/lock"
    fi

    # ── Ensure systemd unit (if present) points at the current bundle path ──────
    # The unit is skipped on first write but never updated in-place, so a
    # stale unit pointing at a different bundle path will keep failing.
    UNIT_FILE="$HOME/.config/systemd/user/containerd.service"
    if [ -f "$UNIT_FILE" ] && ! grep -qF "$ROOTLESS" "$UNIT_FILE"; then
        echo "==> Systemd unit has stale bundle path — reinstalling..."
        "$SETUP" uninstall 2>/dev/null || true
    fi

    # ── LosOS (actman): register containerd-rootless as an actman init service ─
    if [ -f /etc/isoman.json ] || [ -x /bin/updman ]; then
        echo "==> LosOS (actman) detected — writing /etc/init/start/containerd-rootless"
        mkdir -p /etc/init/start
        printf '#!/bin/busybox sh\nexport XDG_RUNTIME_DIR=/run/user/$(id -u)\nexec "%s"\n' \
            "$ROOTLESS" > /etc/init/start/containerd-rootless
        chmod +x /etc/init/start/containerd-rootless
    fi

    # ── Patch setup script: inject actman guard before the first systemd check ─
    PATCHED="{{ nerdctl_bundle }}/bin/containerd-rootless-setuptool-losos.sh"
    awk '
        /systemctl|\/run\/systemd|openrc/ && !injected {
            print "  # LosOS/actman — skip service-manager detection on actman systems"
            print "  if [ -f /etc/isoman.json ] || [ -x /bin/updman ]; then"
            print "    echo \"==> LosOS (actman): containerd-rootless init service registered\""
            print "    return 0"
            print "  fi"
            injected = 1
        }
        { print }
    ' "$SETUP" > "$PATCHED"
    chmod +x "$PATCHED"

    echo "==> Running containerd-rootless-setuptool.sh install (LosOS-patched)..."
    "$PATCHED" install || echo "==> WARNING: setup script returned non-zero — will attempt manual start"

    # Configure nerdctl: point at bundle CNI plugins (nerdctl 2.x dropped
    # default_runtime from toml; runtime is set per-command via --runtime flag).
    mkdir -p "${XDG_CONFIG_HOME:-$HOME/.config}/nerdctl"
    printf 'cni_path = "%s/libexec/cni"\n' "{{ nerdctl_bundle }}" \
        > "${XDG_CONFIG_HOME:-$HOME/.config}/nerdctl/nerdctl.toml"
    echo "==> nerdctl: cni_path = {{ nerdctl_bundle }}/libexec/cni"

    # ── Wait for socket; fall back to a background manual start ───────────────
    for i in $(seq 10); do [ -S "$SOCK" ] && break; sleep 1; done
    [ -S "$SOCK" ] && { echo "==> rootless containerd is up ($SOCK)"; exit 0; }

    echo "==> Starting rootless containerd in the background..."
    nohup "$ROOTLESS" >"{{ nerdctl_bundle }}/containerd.log" 2>&1 &
    disown
    for i in $(seq 20); do
        [ -S "$SOCK" ] && { echo "==> rootless containerd is up ($SOCK)"; exit 0; }
        sleep 1
    done
    echo "ERROR: containerd did not start within 20 s" >&2
    tail -20 "{{ nerdctl_bundle }}/containerd.log" >&2
    exit 1

# Ensure rootless buildkitd is running (needed by nerdctl build).
# Runs buildkitd inside the same rootlesskit user namespace as containerd so it
# can use the containerd worker (shared image store, no second OCI layer).
_ensure-buildkit: _ensure-containerd-rootless
    #!/bin/sh
    set -eu
    export PATH="{{ nerdctl_bundle }}/bin:${PATH}"
    BSOCK="/run/user/$(id -u)/buildkit/buildkitd.sock"
    SETUP="{{ nerdctl_bundle }}/bin/containerd-rootless-setuptool.sh"
    # Quick liveness probe: buildctl exits 0 if the daemon is reachable.
    if [ -S "$BSOCK" ] && buildctl --addr "unix://$BSOCK" debug info >/dev/null 2>&1; then
        echo "==> rootless buildkitd is up ($BSOCK)"
        exit 0
    fi
    [ -S "$BSOCK" ] && { echo "==> Stale buildkitd socket — removing..."; rm -f "$BSOCK"; }
    mkdir -p "$(dirname "$BSOCK")"
    echo "==> Starting rootless buildkitd (containerd worker, nsenter into rootlesskit ns)..."
    # nsenter subcommand enters the containerd rootlesskit user namespace so
    # buildkitd runs as the mapped root — same mechanism used by the systemd unit.
    nohup "$SETUP" nsenter -- buildkitd \
        --addr "unix://$BSOCK" \
        --oci-worker=false \
        --containerd-worker=true \
        --containerd-worker-rootless=true \
        >"{{ nerdctl_bundle }}/buildkitd.log" 2>&1 &
    disown
    for i in $(seq 20); do
        buildctl --addr "unix://$BSOCK" debug info >/dev/null 2>&1 \
            && { echo "==> rootless buildkitd is up ($BSOCK)"; exit 0; }
        sleep 1
    done
    echo "ERROR: buildkitd did not start within 20 s" >&2
    tail -20 "{{ nerdctl_bundle }}/buildkitd.log" >&2
    exit 1

# Build the isoman binary inside a Rust:Alpine container — no host Rust toolchain needed.
# The result is cached at {{ isoman_bin }}; delete it to force a rebuild.
_build-isoman: _ensure-buildkit
    #!/bin/sh
    set -eu
    export PATH="{{ nerdctl_bundle }}/bin:${PATH}"
    out="{{ isoman_bin }}"
    mkdir -p "$(dirname "$out")"
    if [ -x "$out" ]; then
        echo "==> isoman binary found at $out (delete to rebuild)"
        exit 0
    fi
    echo "==> Building isoman inside Rust:Alpine container..."
    "{{ nerdctl_bin }}" build \
        --no-cache \
        -f Containerfile.isoman \
        -t losos-isoman-build \
        "{{ pwd }}"
    CID=$("{{ nerdctl_bin }}" create losos-isoman-build)
    "{{ nerdctl_bin }}" cp "$CID:/isoman" "$out"
    "{{ nerdctl_bin }}" rm "$CID"
    chmod +x "$out"
    echo "==> isoman binary at $out"

# Generate Secure Boot keys only when sb-key.pem / sb-cert.pem are absent.
_ensure-sb-keys:
    #!/bin/sh
    set -eu
    if [ ! -f sb-key.pem ] || [ ! -f sb-cert.pem ]; then
        echo "==> Secure Boot keys not found — generating via sbctl..."
        just setup-sbctl
    fi

# ── Public recipes ────────────────────────────────────────────────────────────

# Pull all submodules to their latest remote commit and build the workspace
dev:
    #!/bin/sh
    set -eu
    echo "==> Pulling submodules..."
    git submodule update --remote --merge
    echo "==> Vendoring dependencies..."
    cargo vendor
    echo "==> Installing pre-commit hook..."
    hook_src="$(pwd)/scripts/pre-commit"
    git_dir=$(git rev-parse --git-dir)
    mkdir -p "$git_dir/hooks"
    cp "$hook_src" "$git_dir/hooks/pre-commit"
    chmod +x "$git_dir/hooks/pre-commit"
    echo "    installed → $git_dir/hooks/pre-commit"
    HOOK_SRC="$hook_src" git submodule foreach --quiet '
        git_dir=$(git rev-parse --git-dir)
        mkdir -p "$git_dir/hooks"
        cp "$HOOK_SRC" "$git_dir/hooks/pre-commit"
        chmod +x "$git_dir/hooks/pre-commit"
        echo "    installed → $git_dir/hooks/pre-commit"
    '
    echo "==> Pre-commit hooks installed"

# Download and build the latest LLVM (clang + lld) from source.
llvm:
    #!/bin/sh
    set -eu

    repo_root="`pwd`"
    cache_root="${BUILD_CACHE:-{{ build_cache }}}"
    case "$cache_root" in /*) ;; *) cache_root="$repo_root/$cache_root" ;; esac
    bootstrap_root="${LLVM_BOOTSTRAP_ROOT:-$cache_root/llvm-bootstrap}"
    stage2_root="${LLVM_STAGE2_ROOT:-$cache_root/llvm-stage2}"
    case "$bootstrap_root" in /*) ;; *) bootstrap_root="$repo_root/$bootstrap_root" ;; esac
    case "$stage2_root" in /*) ;; *) stage2_root="$repo_root/$stage2_root" ;; esac
    install_dir="$repo_root/llvm"
    generator="${GENERATOR:-}"

    if [ -f "$install_dir/bin/clang" ]; then
        echo "==> LLVM already built at $install_dir"
        exit 0
    fi

    echo "==> Checking dependencies..."
    for cmd in curl tar cmake; do
        if ! command -v "$cmd" >/dev/null 2>&1; then
            echo "Error: $cmd is required but not installed."
            exit 1
        fi
    done

    if [ -z "$generator" ]; then
        if command -v ninja >/dev/null 2>&1; then
            generator="Ninja"
        else
            generator="Unix Makefiles"
        fi
    fi

    echo "==> Downloading LLVM source"
    rm -rf "$bootstrap_root" "$stage2_root"
    mkdir -p "$(dirname "$bootstrap_root")" "$(dirname "$stage2_root")"
    mkdir -p "$bootstrap_root"
    # Fetch LLVM
    URL=$(curl -s https://api.github.com/repos/llvm/llvm-project/releases/latest | grep tarball_url | head -n1 | cut -d '"' -f4)
    curl -L "$URL" -o "$bootstrap_root/llvm.tar.gz"
    mkdir -p "$bootstrap_root/src"
    tar -xzf "$bootstrap_root/llvm.tar.gz" -C "$bootstrap_root/src" --strip-components=1
    export CC="clang"
    export CXX="clang++"
    set -- \
        -S "$bootstrap_root/src/llvm" \
        -B "$bootstrap_root/build" \
        -G "$generator" \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_INSTALL_PREFIX="$bootstrap_root/install" \
        -DCMAKE_C_COMPILER=clang \
        -DCMAKE_CXX_COMPILER=clang++ \
        -DLLVM_ENABLE_PROJECTS="clang;lld" \
        -DLLVM_ENABLE_RUNTIMES="libcxx;libcxxabi;libunwind;compiler-rt" \
        -DCOMPILER_RT_DEFAULT_TARGET_ONLY=ON \
        -DLLVM_TARGETS_TO_BUILD="X86" \
        -DLLVM_INCLUDE_TESTS=OFF \
        -DLLVM_INCLUDE_EXAMPLES=OFF \
        -DLLVM_ENABLE_BINDINGS=OFF \
        -DLLVM_USE_LINKER=mold
    if command -v ccache >/dev/null 2>&1; then
        set -- "$@" \
            -DCMAKE_C_COMPILER_LAUNCHER=ccache \
            -DCMAKE_CXX_COMPILER_LAUNCHER=ccache
    fi
    cmake "$@"
    cmake --build "$bootstrap_root/build" -j`nproc`
    cmake --install "$bootstrap_root/build" --prefix "$bootstrap_root/install"
    if [ ! -x "$bootstrap_root/install/bin/clang" ]; then
        echo "Error: bootstrap clang not found at $bootstrap_root/install/bin/clang" >&2
        exit 1
    fi

    echo "==> Building branded LosOS LLVM toolchain (Stage 2)..."
    mkdir -p "$stage2_root/build"
    export CC="$bootstrap_root/install/bin/clang"
    export CXX="$bootstrap_root/install/bin/clang++"

    # Verify the bootstrap compiler can link a trivial C program before invoking
    # cmake — if this fails every cmake feature check will silently fail too.
    printf 'int main(void){return 0;}' > /tmp/_losos_sanity.c
    if ! "$bootstrap_root/install/bin/clang" /tmp/_losos_sanity.c -o /tmp/_losos_sanity 2>&1; then
        echo "ERROR: bootstrap clang cannot link a trivial C program — check CRT / gcc toolchain" >&2
        rm -f /tmp/_losos_sanity.c /tmp/_losos_sanity; exit 1
    fi
    # Also check -stdlib=libc++ to decide whether LLVM_ENABLE_LIBCXX is usable.
    printf 'int main(){return 0;}' > /tmp/_losos_sanity.cpp
    if "$bootstrap_root/install/bin/clang++" -stdlib=libc++ \
            "-L$bootstrap_root/install/lib" /tmp/_losos_sanity.cpp \
            -o /tmp/_losos_sanity 2>&1; then
        llvm_enable_libcxx=ON
        echo "==> bootstrap libc++ OK — will build stage2 with LLVM_ENABLE_LIBCXX=ON"
    else
        llvm_enable_libcxx=OFF
        echo "==> bootstrap libc++ NOT accessible — stage2 will use host libstdc++ (LLVM_ENABLE_LIBCXX=OFF)"
    fi
    rm -f /tmp/_losos_sanity.c /tmp/_losos_sanity.cpp /tmp/_losos_sanity

    # compiler-rt builtins live under clang's resource directory; expose them so
    # cmake try_compile tests can resolve __atomic_* without falling back to libatomic.
    # Modern LLVM uses a triple-based subdirectory (e.g. lib/x86_64-unknown-linux-gnu/)
    # so search both naming conventions.
    res_dir="$("$bootstrap_root/install/bin/clang" -print-resource-dir)"
    compiler_rt_lib=""
    for d in "$res_dir/lib/linux" "$res_dir/lib/x86_64-unknown-linux-gnu" "$res_dir/lib"; do
        if [ -d "$d" ]; then compiler_rt_lib="$d"; break; fi
    done
    # LIBRARY_PATH (link-time search) and LD_LIBRARY_PATH (runtime loading) let
    # cmake try_compile tests find bootstrap libc++.so when -stdlib=libc++ is
    # active.  CMAKE_EXE/SHARED_LINKER_FLAGS carries -L into cmake try_compile
    # sub-projects that reset the environment.
    if [ -n "$compiler_rt_lib" ]; then
        export LIBRARY_PATH="$compiler_rt_lib:$bootstrap_root/install/lib${LIBRARY_PATH:+:$LIBRARY_PATH}"
        export LD_LIBRARY_PATH="$compiler_rt_lib:$bootstrap_root/install/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
        extra_linker_flags="-L$compiler_rt_lib -L$bootstrap_root/install/lib"
    else
        export LIBRARY_PATH="$bootstrap_root/install/lib${LIBRARY_PATH:+:$LIBRARY_PATH}"
        export LD_LIBRARY_PATH="$bootstrap_root/install/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
        extra_linker_flags="-L$bootstrap_root/install/lib"
    fi

    set -- \
        -S "$bootstrap_root/src/llvm" \
        -B "$stage2_root/build" \
        -G "$generator" \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_INSTALL_PREFIX="$install_dir" \
        -DCMAKE_C_COMPILER="$bootstrap_root/install/bin/clang" \
        -DCMAKE_CXX_COMPILER="$bootstrap_root/install/bin/clang++" \
        -DLLVM_ENABLE_PROJECTS="clang;lld" \
        -DLLVM_ENABLE_RUNTIMES="libcxx;libcxxabi;libunwind;compiler-rt" \
        -DCOMPILER_RT_DEFAULT_TARGET_ONLY=ON \
        "-DLLVM_ENABLE_LIBCXX=$llvm_enable_libcxx" \
        -DCLANG_DEFAULT_STDLIB=libc++ \
        -DCLANG_DEFAULT_RTLIB=compiler-rt \
        -DCLANG_DEFAULT_UNWINDLIB=libunwind \
        "-DCMAKE_INSTALL_RPATH=$install_dir/lib" \
        "-DCMAKE_BUILD_RPATH=$bootstrap_root/install/lib" \
        -DCMAKE_BUILD_WITH_INSTALL_RPATH=OFF \
        "-DCMAKE_EXE_LINKER_FLAGS=$extra_linker_flags" \
        "-DCMAKE_SHARED_LINKER_FLAGS=$extra_linker_flags" \
        -DLLVM_TARGETS_TO_BUILD="X86" \
        -DLLVM_INCLUDE_TESTS=OFF \
        -DLLVM_INCLUDE_EXAMPLES=OFF \
        -DLLVM_ENABLE_BINDINGS=OFF \
        -DCLANG_VENDOR="LosOS" \
        -DPACKAGE_VENDOR="LosOS" \
        -DLLVM_ENABLE_LTO=Thin \
        -DLLVM_PARALLEL_LINK_JOBS=1 \
        -DLLVM_USE_LINKER="$bootstrap_root/install/bin/ld.lld" \
        -DLLVM_ENABLE_LIBXML2=OFF \
        "-DCMAKE_C_FLAGS=-fvisibility=hidden -fvisibility-inlines-hidden" \
        "-DCMAKE_CXX_FLAGS=-fvisibility=hidden -fvisibility-inlines-hidden"
    cmake "$@" || {
        printf '\n==> Stage2 CMake configure failed.  Configure log (last 60 lines):\n'
        tail -60 "$stage2_root/build/CMakeFiles/CMakeConfigureLog.yaml" 2>/dev/null || true
        exit 1
    }
    cmake --build "$stage2_root/build" -j`nproc`
    cmake --install "$stage2_root/build" --prefix "$install_dir"

    echo "==> Clean up build artifacts..."
    rm -rf "$bootstrap_root" "$stage2_root"

# Build a custom kernel optimised for LosOS inside an isolated container.
# Requires: Containerfile.kernel image (built automatically on first run).
kernel: llvm _ensure-buildkit
    #!/bin/sh
    set -eu
    export PATH="{{ nerdctl_bundle }}/bin:${PATH}"

    repo_root="`pwd`"
    cache_root="${BUILD_CACHE:-{{ build_cache }}}"
    case "$cache_root" in /*) ;; *) cache_root="$repo_root/$cache_root" ;; esac
    build_root="${KERNEL_BUILD_ROOT:-$cache_root/kernel}"
    case "$build_root" in /*) ;; *) build_root="$repo_root/$build_root" ;; esac

    tag="{{ kernel_tag }}"
    archive="${build_root}/${tag}.tar.gz"
    src_dir="${build_root}/linux-${tag#v}"

    echo "==> Ensuring kernel build environment image..."
    "{{ nerdctl_bin }}" build -q -f Containerfile.kernel -t losos-kernel-build "$repo_root"

    echo "==> Pulling kernel ${tag}"
    rm -rf "$build_root"
    mkdir -p "$build_root"
    curl -fL "https://github.com/torvalds/linux/archive/refs/tags/${tag}.tar.gz" -o "$archive"
    tar -xzf "$archive" -C "$build_root"

    afdo_env=""
    propeller_env=""
    afdo_prof="${AUTOFDO_PROFILE:-}"
    [ -z "$afdo_prof" ] && [ -f "$cache_root/kernel.afdo" ] && afdo_prof="$cache_root/kernel.afdo"
    propeller_prefix="${PROPELLER_PREFIX:-}"
    [ -z "$propeller_prefix" ] && [ -f "$cache_root/kernel-propeller.symorder" ] \
        && propeller_prefix="$cache_root/kernel-propeller"
    if [ -n "$afdo_prof" ]; then
        echo "==> AutoFDO profile: $afdo_prof"
        afdo_env="CLANG_AUTOFDO_PROFILE=/cache/kernel.afdo"
    fi
    if [ -n "$propeller_prefix" ]; then
        echo "==> Propeller prefix: $propeller_prefix"
        propeller_env="CLANG_PROPELLER_PROFILE_PREFIX=/cache/kernel-propeller"
    fi

    mkdir -p "$cache_root/ccache-kernel"

    echo "==> Building kernel ${tag} in container..."
    # Mount host libxml2 so that the host-built ld.lld (which links against
    # libxml2.so.16, bumped from .so.2 in libxml2 2.13) can load it inside
    # the container whose distro still ships the older soname.
    host_xml2=$(readlink -f /usr/lib/libxml2.so.16 2>/dev/null || true)
    xml2_mounts=""
    if [ -n "$host_xml2" ]; then
        xml2_mounts="-v /usr/lib/libxml2.so.16:/usr/lib/libxml2.so.16:ro -v ${host_xml2}:${host_xml2}:ro"
    fi
    "{{ nerdctl_bin }}" run --rm -i \
        -v "$repo_root/llvm:/llvm:ro" \
        -v "$build_root:/src" \
        -v "$cache_root:/cache" \
        -v "$cache_root/ccache-kernel:/ccache" \
        -v "$repo_root:/repo:ro" \
        ${xml2_mounts} \
        -e KERNEL_TAG="$tag" \
        -e AFDO_ENV="$afdo_env" \
        -e PROPELLER_ENV="$propeller_env" \
        losos-kernel-build sh <<'KERNELBUILD'
    set -eu
    export CCACHE_DIR=/ccache
    export CCACHE_COMPILERCHECK=content
    export PATH="/llvm/bin:$PATH"
    cd "/src/linux-${KERNEL_TAG#v}"
    make tinyconfig LLVM=1
    ./scripts/config \
        -e 64BIT -e BLK_DEV_INITRD -e RD_GZIP -e BINFMT_ELF -e BINFMT_SCRIPT \
        -e PRINTK -e EARLY_PRINTK -e TTY -e SERIAL_8250 -e SERIAL_8250_CONSOLE \
        -e PCI -e VIRTUALIZATION -e KVM -e KVM_INTEL -e KVM_AMD \
        -e VIRTIO -e VIRTIO_PCI -e VIRTIO_BLK -e VIRTIO_NET -e VIRTIO_VSOCK -e VHOST_VSOCK \
        -e BLOCK -e BLK_DEV_SD -e BLK_DEV_DM -e DM_CRYPT -e DM_INTEGRITY -e DM_VERITY \
        -e CRYPTO_AES_X86_64 -e CRYPTO_SHA256 -e CRYPTO_USER_API_SKCIPHER -e CRYPTO_USER_API_HASH \
        -e NET -e INET -e NETDEVICES -e NAMESPACES -e UTS_NS -e IPC_NS -e USER_NS -e PID_NS -e NET_NS \
        -e EFI -e EFIVAR_FS -e ISO9660_FS -e TMPFS -e DEVTMPFS -e DEVTMPFS_MOUNT \
        -e RELOCATABLE -e RANDOMIZE_BASE -e RELR \
        -e LTO_CLANG_FULL -e CFI_CLANG -e CC_OPTIMIZE_FOR_SIZE -e AUTOFDO_CLANG -e PROPELLER_CLANG \
        -e SECURITY_LANDLOCK -e BPF_SYSCALL \
        -e MODULES -e MODULE_SIG -e MODULE_SIG_ALL -e MODULE_SIG_FORCE -e MODULE_SIG_SHA256 \
        --set-str LOCALVERSION "-losos" \
        --set-str DEFAULT_HOSTNAME "losos" \
        --set-str MODULE_SIG_KEY "/repo/sb-key.pem" \
        --set-str MODULE_SIG_CERT "/repo/sb-cert.pem"
    make olddefconfig LLVM=1
    # LTO_CLANG_FULL requires LD_IS_LLD; mold won't satisfy that check.
    # Absolute compiler path so ccache's content-hash check is unambiguous.
    # ${VAR:+$VAR} expands to KEY=VALUE (no spaces) or nothing — safe unquoted.
    make LLVM=1 LD=ld.lld \
        CC="ccache /llvm/bin/clang" \
        CXX="ccache /llvm/bin/clang++" \
        HOSTCC="ccache /llvm/bin/clang" \
        HOSTCXX="ccache /llvm/bin/clang++" \
        KCFLAGS="-fsanitize=cfi -fvisibility=hidden -fvisibility-inlines-hidden" \
        HOSTCFLAGS="-fsanitize=cfi -fvisibility=hidden -fvisibility-inlines-hidden" \
        HOSTCXXFLAGS="-fsanitize=cfi -fvisibility=hidden -fvisibility-inlines-hidden" \
        ${AFDO_ENV:+$AFDO_ENV} \
        ${PROPELLER_ENV:+$PROPELLER_ENV} \
        -j$(nproc)
    echo "==> Signing kernel bzImage..."
    if [ -f /repo/sb-key.pem ] && [ -f /repo/sb-cert.pem ]; then
        sbsign --key /repo/sb-key.pem --cert /repo/sb-cert.pem \
            --output arch/x86/boot/bzImage arch/x86/boot/bzImage
    else
        echo "WARNING: Secure Boot keys not found — kernel image will be unsigned."
    fi
    KERNELBUILD

    cp "$src_dir/arch/x86/boot/bzImage" "$repo_root/vmlinuz"

# Collect AutoFDO samples for kernel PGO.  Boots the ISO under perf-kvm to
# capture guest branch samples, converts them to an AFDO profile, then the
# next 'just kernel' build picks it up automatically from the cache.
kernel-profiles: kernel
    #!/bin/sh
    set -eu

    export PATH="`pwd`/llvm/bin:$PATH"

    repo_root="`pwd`"
    cache_root="${BUILD_CACHE:-{{ build_cache }}}"
    case "$cache_root" in /*) ;; *) cache_root="$repo_root/$cache_root" ;; esac
    build_root="${KERNEL_BUILD_ROOT:-$cache_root/kernel}"
    case "$build_root" in /*) ;; *) build_root="$repo_root/$build_root" ;; esac

    tag="{{ kernel_tag }}"
    src_dir="${build_root}/linux-${tag#v}"
    vmlinux="$src_dir/vmlinux"

    if [ ! -f "{{ output }}" ]; then
        echo "==> No ISO at {{ output }} yet — skipping profile collection (will run after first build)."
        exit 0
    fi

    perf_data="$cache_root/kernel-perf.data"
    afdo_out="$cache_root/kernel.afdo"
    mkdir -p "$cache_root"

    echo "==> Booting ISO under perf-kvm for ~60 s to collect AutoFDO branch samples..."
    tmp_vars=$(mktemp --suffix=.fd)
    cp "{{ ovmf_vars }}" "$tmp_vars"
    trap "rm -f $tmp_vars" EXIT

    timeout 60 perf kvm --guest record \
        -e cycles -b \
        -o "$perf_data" -- \
        qemu-system-x86_64 \
            -cpu host \
            -m "{{ memory }}" \
            -smp "{{ cpus }}" \
            -enable-kvm \
            -drive "if=pflash,format=raw,readonly=on,file={{ ovmf_code }}" \
            -drive "if=pflash,format=raw,file=$tmp_vars" \
            -drive "file={{ output }},format=raw,media=cdrom,readonly=on" \
            -nographic \
            -netdev user,id=n0 \
            -device virtio-net-pci,netdev=n0 \
        || true

    echo "==> Converting perf data → AutoFDO profile..."
    llvm-profgen --kernel \
        --perfdata="$perf_data" \
        --binary="$vmlinux" \
        --output="$afdo_out"

    echo "==> Profile written to $afdo_out"
    echo "    Run 'just kernel' to rebuild the kernel with AutoFDO PGO applied."

# Generate a full UEFI Secure Boot key hierarchy (PK, KEK, db) via sbctl in an
# ephemeral Arch Linux container.  The db signing key/cert are copied to
# sb-key.pem / sb-cert.pem in the project root where isoman --secure-boot
# picks them up automatically.  The full hierarchy lives in secure-boot/.
setup-sbctl:
    #!/bin/sh
    set -eu
    export PATH="{{ nerdctl_bundle }}/bin:${PATH}"

    SB_DIR="`pwd`/secure-boot"
    mkdir -p "$SB_DIR"

    echo "==> Generating Secure Boot key hierarchy via sbctl (ephemeral Alpine container)..."
    "{{ nerdctl_bin }}" run --rm --runtime=krun \
        --name sbctl-setup \
        -v "$SB_DIR:/usr/share/secureboot" \
        docker.io/library/alpine:latest \
        sh -c "
            apk add --no-cache --repository=https://dl-cdn.alpinelinux.org/alpine/edge/community sbctl 2>&1 | tail -3
            sbctl create-keys
        "

    # The db key/cert are what sbsign (isoman --secure-boot) needs.
    cp "$SB_DIR/keys/db/db.key" sb-key.pem
    cp "$SB_DIR/keys/db/db.pem" sb-cert.pem
    chmod 600 sb-key.pem

    echo ""
    echo "==> Full key hierarchy written to secure-boot/"
    echo "    PK  : secure-boot/keys/PK/PK.{key,pem}"
    echo "    KEK : secure-boot/keys/KEK/KEK.{key,pem}"
    echo "    db  : secure-boot/keys/db/db.{key,pem}"
    echo ""
    echo "==> Signing key/cert copied to sb-key.pem / sb-cert.pem"
    echo "    Run 'just build-secure-boot' to sign the EFI binary with these keys."

# Launch initramfs in QEMU (default)
default: build-secure-boot run

# Build OS ISO image — isoman built in Alpine, full container assembly (no host Rust toolchain needed)
build: _dm-integrity _build-isoman kernel-profiles
    @echo "==> Building OS ISO image (Alpine container, full nerdctl assembly)..."
    "{{ isoman_bin }}" --build --no-host-compile --output "{{ output }}"
    @echo "==> ISO image written to {{ output }}"

# Build OS ISO image with host-compiled Rust (requires cargo; faster incremental rebuilds)
container-build: _dm-integrity _ensure-containerd-rootless kernel-profiles
    @echo "==> Building OS ISO image (host-compiled Rust, nerdctl cpio assembly)..."
    cargo run -p isoman -- --build --output "{{ output }}" --kernel "{{ kernel }}"
    @echo "==> ISO image written to {{ output }}"

# Build using a JSON config file (ISOMAN_CONFIG env or explicit path)
build-config config_path=isoman_config: _dm-integrity _build-isoman kernel-profiles
    @echo "==> Building from config: {{ config_path }}"
    "{{ isoman_bin }}" --build --config "{{ config_path }}" --output "{{ output }}"

# Build a GSI (Fastboot + Odin) instead of a bootable ISO
build-gsi: _build-isoman kernel-profiles
    @echo "==> Building GSI (Fastboot + Odin)..."
    "{{ isoman_bin }}" --build --gsi

# Build a Fastboot-only GSI boot.img
build-gsi-fastboot: _build-isoman kernel-profiles
    "{{ isoman_bin }}" --build --gsi --gsi-fastboot

# Build production-hardened OS image (loglevel=0 + security mitigations)
build-prod: _dm-integrity _ensure-sb-keys _build-isoman kernel-profiles
    @echo "==> Building production OS disk image (hardened cmdline)..."
    "{{ isoman_bin }}" --build --profile prod --kernel "{{ kernel }}"
    @echo "==> Production disk image written to os-<mode>.img"

# Build production live OS image (hardened cmdline + container-ready for preflight)
build-prod-live: _dm-integrity _ensure-sb-keys _build-isoman kernel-profiles
    @echo "==> Building production live OS disk image (container-ready for preflight)..."
    "{{ isoman_bin }}" --build --profile prod-live --kernel "{{ kernel }}"
    @echo "==> Production live disk image written to os-<mode>.img"

# Build with Secure Boot signing (auto-generates sb-key.pem / sb-cert.pem if absent)
build-secure-boot: _dm-integrity _ensure-sb-keys _build-isoman kernel-profiles
    #!/bin/sh
    set -eu
    echo "==> Building with Secure Boot signing..."
    kernel_arg=""
    [ -n "{{ kernel }}" ] && kernel_arg="--kernel {{ kernel }}"
    "{{ isoman_bin }}" --build --output "{{ output }}" --secure-boot true $kernel_arg

# Build initramfs then launch in QEMU
build-run: build run

# Build initramfs then run integration tests
build-test: build test

# Run testman integration tests in legacy BIOS mode (El Torito, no OVMF)
test-bios: _ensure-nextest
    #!/bin/sh
    set -eu

    echo "==> Running testman integration tests (BIOS/El Torito mode)"
    echo "    ISO:         {{ output }}"
    echo "    Memory:      {{ memory }}"
    echo "    CPUs:        {{ cpus }}"
    echo ""

    exec env 
        ISO="{{ output }}" 
        MEMORY="{{ memory }}" 
        CPUS="{{ cpus }}" 
        KVM="{{ kvm }}" 
        BIOS=1 
        cargo nextest run --manifest-path crates/testman/Cargo.toml --test-threads 1 --run-ignored all

# Run testman integration tests (builds non-prod encrypted disk image first)
test: _ensure-nextest
    #!/bin/sh
    set -eu

    echo "==> Running testman integration tests"
    echo "    ISO:         {{ output }}"
    echo "    Memory:      {{ memory }}"
    echo "    CPUs:        {{ cpus }}"
    echo ""

    exec env 
        ISO="{{ output }}" 
        MEMORY="{{ memory }}" 
        CPUS="{{ cpus }}" 
        KVM="{{ kvm }}" 
        OVMF_CODE="{{ ovmf_code }}" 
        OVMF_VARS="{{ ovmf_vars }}" 
        cargo nextest run --manifest-path crates/testman/Cargo.toml --test-threads 1 --run-ignored all

# Launch OS ISO image in QEMU via UEFI (OVMF pflash + virtio drive)
run:
    #!/bin/sh
    set -eu

    echo "==> Launching OS ISO image (UEFI)"
    echo "    Image:     {{ output }}"
    echo "    Memory:    {{ memory }}"
    echo "    CPUs:      {{ cpus }}"
    [ -n "{{ disk }}" ] && echo "    Data disk: {{ disk }} (→ /dev/vdb)"
    echo ""

    # OVMF_VARS must be writable; use a temp copy to avoid mutating the
    # system-wide file.
    tmp_vars=$(mktemp /tmp/ovmf-XXXXXX.fd)
    cp "{{ ovmf_vars }}" "$tmp_vars"
    trap 'rm -f "$tmp_vars"' EXIT

    set -- \
        qemu-system-x86_64 \
        -m "{{ memory }}" \
        -smp "{{ cpus }}" \
        -drive "if=pflash,format=raw,readonly=on,file={{ ovmf_code }}" \
        -drive "if=pflash,format=raw,file=$tmp_vars" \
        -drive "file={{ output }},format=raw,media=cdrom,readonly=on" \
        -nographic \
        -nic user \
        -netdev user,id=n0 \
        -device virtio-net-pci,netdev=n0
    # Extra data disk (e.g. for persistent storage).  OS image is /dev/vda;
    # the optional data disk appears as /dev/vdb.
    [ -n "{{ disk }}" ] && set -- "$@" -drive "file={{ disk }},format=raw,if=virtio"
    [ "{{ kvm }}" -eq 1 ] && set -- "$@" -enable-kvm
    exec "$@"

# Launch OS ISO image in QEMU via Legacy BIOS
run-bios:
    #!/bin/sh
    set -eu

    echo "==> Launching OS ISO image (Legacy BIOS)"
    echo "    Image:     {{ output }}"
    echo "    Memory:    {{ memory }}"
    echo "    CPUs:      {{ cpus }}"
    [ -n "{{ disk }}" ] && echo "    Data disk: {{ disk }} (→ /dev/vdb)"
    echo ""

    set -- \
        qemu-system-x86_64 \
        -m "{{ memory }}" \
        -smp "{{ cpus }}" \
        -drive "file={{ output }},format=raw,media=cdrom,readonly=on" \
        -nographic \
        -nic user \
        -netdev user,id=n0 \
        -device virtio-net-pci,netdev=n0
    [ -n "{{ disk }}" ] && set -- "$@" -drive "file={{ disk }},format=raw,if=virtio"
    [ "{{ kvm }}" -eq 1 ] && set -- "$@" -enable-kvm
    exec "$@"
