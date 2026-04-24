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
# nerdctl binary: prefer system install, fall back to bootstrap copy in /tmp.
nerdctl_bin := `command -v nerdctl 2>/dev/null || echo /tmp/nerdctl-bin/nerdctl`
# Pre-built isoman binary (built inside Alpine by _build-isoman).
isoman_bin := env("ISOMAN_BIN", ".build-cache/isoman")
# ── Private helpers ───────────────────────────────────────────────────────────

# Ensure cargo-nextest is installed
_ensure-nextest:
    #!/usr/bin/env bash
    if ! command -v cargo-nextest &> /dev/null; then
        echo "==> cargo-nextest not found — installing..."
        cargo install cargo-nextest --locked
    fi

# Load dm-integrity and ensure root access for cryptsetup loop-device operations.
_dm-integrity:
    #!/usr/bin/env bash
    if ! lsmod | grep -q "^dm_integrity" && ! grep -q "^dm_integrity" /proc/modules; then
        sudo modprobe dm-integrity || echo "WARNING: dm-integrity unavailable (restricted environment) — continuing."
    fi

# Download the static nerdctl binary to /tmp/nerdctl-bin/ when nerdctl is not
# already installed on the host.  All recipes that invoke nerdctl depend on this.
_ensure-nerdctl:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -x "{{ nerdctl_bin }}" ]; then
        exit 0
    fi
    echo "==> nerdctl not found — downloading static binary to /tmp/nerdctl-bin/..."
    mkdir -p /tmp/nerdctl-bin
    API=$(curl -fsSL https://api.github.com/repos/containerd/nerdctl/releases/latest)
    # Match lines that contain a download URL ending in linux-amd64.tar.gz, excluding the
    # "full" bundle (which bundles containerd, buildkit, etc. and is much larger).
    URL=$(printf '%s\n' "$API" \
        | grep 'browser_download_url' \
        | grep 'linux-amd64\.tar\.gz' \
        | grep -v 'full' \
        | head -1 \
        | cut -d '"' -f4)
    if [ -z "$URL" ]; then
        echo "ERROR: could not find a nerdctl linux-amd64 download URL." >&2
        echo "       GitHub API response (first 10 lines):" >&2
        printf '%s\n' "$API" | head -10 >&2
        exit 1
    fi
    echo "==> Fetching $URL"
    curl -fsSL "$URL" | tar -xz -C /tmp/nerdctl-bin nerdctl
    chmod +x /tmp/nerdctl-bin/nerdctl
    echo "==> nerdctl downloaded to /tmp/nerdctl-bin/nerdctl"

# Build the isoman binary inside a Rust:Alpine container — no host Rust toolchain needed.
# The result is cached at {{ isoman_bin }}; delete it to force a rebuild.
_build-isoman: _ensure-nerdctl
    #!/usr/bin/env bash
    set -euo pipefail
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
    #!/usr/bin/bash
    set -euo pipefail
    if [[ ! -f sb-key.pem || ! -f sb-cert.pem ]]; then
        echo "==> Secure Boot keys not found — generating via sbctl..."
        just setup-sbctl
    fi

# ── Public recipes ────────────────────────────────────────────────────────────

# Pull all submodules to their latest remote commit and build the workspace
dev:
    @echo "==> Pulling submodules..."
    git submodule update --remote --merge

# Download and build the latest LLVM (clang + lld) from source.
llvm:
    #!/usr/bin/env bash
    set -euo pipefail

    repo_root="`pwd`"
    cache_root="${BUILD_CACHE:-{{ build_cache }}}"
    [[ "$cache_root" = /* ]] || cache_root="$repo_root/$cache_root"
    bootstrap_root="${LLVM_BOOTSTRAP_ROOT:-$cache_root/llvm-bootstrap}"
    stage2_root="${LLVM_STAGE2_ROOT:-$cache_root/llvm-stage2}"
    [[ "$bootstrap_root" = /* ]] || bootstrap_root="$repo_root/$bootstrap_root"
    [[ "$stage2_root" = /* ]] || stage2_root="$repo_root/$stage2_root"
    install_dir="$repo_root/llvm"
    generator="${GENERATOR:-}"

    if [[ -f "$install_dir/bin/clang" ]]; then
        echo "==> LLVM already built at $install_dir"
        exit 0
    fi

    echo "==> Checking dependencies..."
    for cmd in curl tar cmake; do
        if ! command -v $cmd &> /dev/null; then
            echo "Error: $cmd is required but not installed."
            exit 1
        fi
    done

    if [[ -z "$generator" ]]; then
        if command -v ninja &> /dev/null; then
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
    BOOTSTRAP_CMAKE_ARGS=(
        -S "$bootstrap_root/src/llvm"
        -B "$bootstrap_root/build"
        -G "$generator"
        -DCMAKE_BUILD_TYPE=Release
        -DCMAKE_INSTALL_PREFIX="$bootstrap_root/install"
        -DCMAKE_C_COMPILER=clang
        -DCMAKE_CXX_COMPILER=clang++
        -DLLVM_ENABLE_PROJECTS="clang;lld"
        -DLLVM_TARGETS_TO_BUILD="X86"
        -DLLVM_INCLUDE_TESTS=OFF
        -DLLVM_INCLUDE_EXAMPLES=OFF
        -DLLVM_ENABLE_BINDINGS=OFF
        -DLLVM_USE_LINKER=mold
    )
    if command -v ccache &>/dev/null; then
        BOOTSTRAP_CMAKE_ARGS+=(
            -DCMAKE_C_COMPILER_LAUNCHER=ccache
            -DCMAKE_CXX_COMPILER_LAUNCHER=ccache
        )
    fi
    cmake "${BOOTSTRAP_CMAKE_ARGS[@]}"
    cmake --build "$bootstrap_root/build" -j`nproc`
    cmake --install "$bootstrap_root/build" --prefix "$bootstrap_root/install"
    if [[ ! -x "$bootstrap_root/install/bin/clang" ]]; then
        echo "Error: bootstrap clang not found at $bootstrap_root/install/bin/clang" >&2
        exit 1
    fi

    echo "==> Building branded LosOS LLVM toolchain (Stage 2)..."
    mkdir -p "$stage2_root/build"
    export CC="$bootstrap_root/install/bin/clang"
    export CXX="$bootstrap_root/install/bin/clang++"

    CMAKE_ARGS=(
        -S "$bootstrap_root/src/llvm"
        -B "$stage2_root/build"
        -G "$generator"
        -DCMAKE_BUILD_TYPE=Release
        -DCMAKE_INSTALL_PREFIX="$install_dir" -DCMAKE_C_COMPILER="$bootstrap_root/install/bin/clang" -DCMAKE_CXX_COMPILER="$bootstrap_root/install/bin/clang++"
        -DLLVM_ENABLE_PROJECTS="clang;lld"
        -DLLVM_TARGETS_TO_BUILD="X86"
        -DLLVM_INCLUDE_TESTS=OFF
        -DLLVM_INCLUDE_EXAMPLES=OFF
        -DLLVM_ENABLE_BINDINGS=OFF
        -DCLANG_VENDOR="LosOS"
        -DPACKAGE_VENDOR="LosOS"
        -DLLVM_ENABLE_LTO=Full
        -DLLVM_ENABLE_CFI=ON
        -DLLVM_USE_LINKER=mold
    )
    cmake "${CMAKE_ARGS[@]}" 
    cmake --build "$stage2_root/build" -j`nproc`
    cmake --install "$stage2_root/build" --prefix "$install_dir"
    
    echo "==> Clean up build artifacts..."
    rm -rf "$bootstrap_root" "$stage2_root"

# Build a custom kernel optimised for LosOS inside an isolated container.
# Requires: Containerfile.kernel image (built automatically on first run).
kernel: llvm _ensure-nerdctl
    #!/usr/bin/env bash
    set -euo pipefail

    repo_root="`pwd`"
    cache_root="${BUILD_CACHE:-{{ build_cache }}}"
    [[ "$cache_root" = /* ]] || cache_root="$repo_root/$cache_root"
    build_root="${KERNEL_BUILD_ROOT:-$cache_root/kernel}"
    [[ "$build_root" = /* ]] || build_root="$repo_root/$build_root"

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
    [[ -z "$afdo_prof" && -f "$cache_root/kernel.afdo" ]] && afdo_prof="$cache_root/kernel.afdo"
    propeller_prefix="${PROPELLER_PREFIX:-}"
    [[ -z "$propeller_prefix" && -f "$cache_root/kernel-propeller.symorder" ]] \
        && propeller_prefix="$cache_root/kernel-propeller"
    [[ -n "$afdo_prof" ]] \
        && { echo "==> AutoFDO profile: $afdo_prof"; afdo_env="CLANG_AUTOFDO_PROFILE=/cache/kernel.afdo"; }
    [[ -n "$propeller_prefix" ]] \
        && { echo "==> Propeller prefix: $propeller_prefix"; propeller_env="CLANG_PROPELLER_PROFILE_PREFIX=/cache/kernel-propeller"; }

    mkdir -p "$cache_root/ccache-kernel"

    echo "==> Building kernel ${tag} in container..."
    "{{ nerdctl_bin }}" run --rm --runtime=krun -i \
        -v "$repo_root/llvm:/llvm:ro" \
        -v "$build_root:/src" \
        -v "$cache_root:/cache" \
        -v "$cache_root/ccache-kernel:/ccache" \
        -v "$repo_root:/repo:ro" \
        -e KERNEL_TAG="$tag" \
        -e AFDO_ENV="$afdo_env" \
        -e PROPELLER_ENV="$propeller_env" \
        losos-kernel-build bash -s <<'KERNELBUILD'
    set -euo pipefail
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
    profile_args=()
    [[ -n "$AFDO_ENV" ]] && profile_args+=("$AFDO_ENV")
    [[ -n "$PROPELLER_ENV" ]] && profile_args+=("$PROPELLER_ENV")
    # LTO_CLANG_FULL requires LD_IS_LLD; mold won't satisfy that check.
    # Absolute compiler path so ccache's content-hash check is unambiguous.
    make LLVM=1 LD=ld.lld \
        CC="ccache /llvm/bin/clang" \
        CXX="ccache /llvm/bin/clang++" \
        HOSTCC="ccache /llvm/bin/clang" \
        HOSTCXX="ccache /llvm/bin/clang++" \
        "${profile_args[@]}" -j$(nproc)
    echo "==> Signing kernel bzImage..."
    if [[ -f /repo/sb-key.pem && -f /repo/sb-cert.pem ]]; then
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
    #!/usr/bin/env bash
    set -euo pipefail

    export PATH="`pwd`/llvm/bin:$PATH"

    repo_root="`pwd`"
    cache_root="${BUILD_CACHE:-{{ build_cache }}}"
    [[ "$cache_root" = /* ]] || cache_root="$repo_root/$cache_root"
    build_root="${KERNEL_BUILD_ROOT:-$cache_root/kernel}"
    [[ "$build_root" = /* ]] || build_root="$repo_root/$build_root"

    tag="{{ kernel_tag }}"
    src_dir="${build_root}/linux-${tag#v}"
    vmlinux="$src_dir/vmlinux"

    if [[ ! -f "{{ output }}" ]]; then
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
    #!/usr/bin/env bash
    set -euo pipefail

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
build: _dm-integrity _build-isoman _ensure-nerdctl kernel-profiles
    @echo "==> Building OS ISO image (Alpine container, full nerdctl assembly)..."
    "{{ isoman_bin }}" --build --no-host-compile --output "{{ output }}"
    @echo "==> ISO image written to {{ output }}"

# Build OS ISO image with host-compiled Rust (requires cargo; faster incremental rebuilds)
container-build: _dm-integrity _ensure-nerdctl kernel-profiles
    @echo "==> Building OS ISO image (host-compiled Rust, nerdctl cpio assembly)..."
    cargo run -p isoman -- --build --output "{{ output }}" --kernel "{{ kernel }}"
    @echo "==> ISO image written to {{ output }}"

# Build using a JSON config file (ISOMAN_CONFIG env or explicit path)
build-config config_path=isoman_config: _dm-integrity _build-isoman _ensure-nerdctl kernel-profiles
    @echo "==> Building from config: {{ config_path }}"
    "{{ isoman_bin }}" --build --config "{{ config_path }}" --output "{{ output }}"

# Build a GSI (Fastboot + Odin) instead of a bootable ISO
build-gsi: _build-isoman _ensure-nerdctl kernel-profiles
    @echo "==> Building GSI (Fastboot + Odin)..."
    "{{ isoman_bin }}" --build --gsi

# Build a Fastboot-only GSI boot.img
build-gsi-fastboot: _build-isoman _ensure-nerdctl kernel-profiles
    "{{ isoman_bin }}" --build --gsi --gsi-fastboot

# Build production-hardened OS image (loglevel=0 + security mitigations)
build-prod: _dm-integrity _ensure-sb-keys _build-isoman _ensure-nerdctl kernel-profiles
    @echo "==> Building production OS disk image (hardened cmdline)..."
    "{{ isoman_bin }}" --build --profile prod --kernel "{{ kernel }}"
    @echo "==> Production disk image written to os-<mode>.img"

# Build production live OS image (hardened cmdline + container-ready for preflight)
build-prod-live: _dm-integrity _ensure-sb-keys _build-isoman _ensure-nerdctl kernel-profiles
    @echo "==> Building production live OS disk image (container-ready for preflight)..."
    "{{ isoman_bin }}" --build --profile prod-live --kernel "{{ kernel }}"
    @echo "==> Production live disk image written to os-<mode>.img"

# Build with Secure Boot signing (auto-generates sb-key.pem / sb-cert.pem if absent)
build-secure-boot: _dm-integrity _ensure-sb-keys _build-isoman _ensure-nerdctl kernel-profiles
    @echo "==> Building with Secure Boot signing..."
    kernel_arg=$([[ -n "{{ kernel }}" ]] && echo "--kernel {{ kernel }}" || echo "");
    "{{ isoman_bin }}" --build --output "{{ output }}" --secure-boot true $kernel_arg

# Build initramfs then launch in QEMU
build-run: build run

# Build initramfs then run integration tests
build-test: build test

# Run testman integration tests in legacy BIOS mode (El Torito, no OVMF)
test-bios: _ensure-nextest
    #!/usr/bin/env bash
    set -euo pipefail

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
    #!/usr/bin/env bash
    set -euo pipefail

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
    #!/usr/bin/env bash
    set -euo pipefail

    echo "==> Launching OS ISO image (UEFI)"
    echo "    Image:     {{ output }}"
    echo "    Memory:    {{ memory }}"
    echo "    CPUs:      {{ cpus }}"
    [[ -n "{{ disk }}" ]] && echo "    Data disk: {{ disk }} (→ /dev/vdb)"
    echo ""

    kvm_flag=()
    [[ "{{ kvm }}" -eq 1 ]] && kvm_flag=(-enable-kvm)

    # Extra data disk (e.g. for persistent storage).  The OS image is /dev/vda;
    # the optional data disk appears as /dev/vdb.
    data_disk_args=()
    [[ -n "{{ disk }}" ]] && data_disk_args=(-drive "file={{ disk }},format=raw,if=virtio")

    # OVMF_VARS must be writable; use a temp copy to avoid mutating the
    # system-wide file.
    tmp_vars=$(mktemp --suffix=.fd)
    cp "{{ ovmf_vars }}" "$tmp_vars"
    trap "rm -f $tmp_vars" EXIT

    exec qemu-system-x86_64 
        -m "{{ memory }}" 
        -smp "{{ cpus }}" 
        -drive "if=pflash,format=raw,readonly=on,file={{ ovmf_code }}" 
        -drive "if=pflash,format=raw,file=$tmp_vars" 
        -drive "file={{ output }},format=raw,media=cdrom,readonly=on" 
        -nographic 
        -nic user 
        -netdev user,id=n0 
        -device virtio-net-pci,netdev=n0 
        "${data_disk_args[@]}" 
        "${kvm_flag[@]}"

# Launch OS ISO image in QEMU via Legacy BIOS
run-bios:
    #!/usr/bin/env bash
    set -euo pipefail

    echo "==> Launching OS ISO image (Legacy BIOS)"
    echo "    Image:     {{ output }}"
    echo "    Memory:    {{ memory }}"
    echo "    CPUs:      {{ cpus }}"
    [[ -n "{{ disk }}" ]] && echo "    Data disk: {{ disk }} (→ /dev/vdb)"
    echo ""

    kvm_flag=()
    [[ "{{ kvm }}" -eq 1 ]] && kvm_flag=(-enable-kvm)

    data_disk_args=()
    [[ -n "{{ disk }}" ]] && data_disk_args=(-drive "file={{ disk }},format=raw,if=virtio")

    exec qemu-system-x86_64 
        -m "{{ memory }}" 
        -smp "{{ cpus }}" 
        -drive "file={{ output }},format=raw,media=cdrom,readonly=on" 
        -nographic 
        -nic user 
        -netdev user,id=n0 
        -device virtio-net-pci,netdev=n0 
        "${data_disk_args[@]}" 
        "${kvm_flag[@]}"
