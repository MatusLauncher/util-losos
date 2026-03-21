#!/usr/bin/env bash
set -euo pipefail

INITRAMFS="os.initramfs.tar.gz"
KERNEL="${KERNEL:-/boot/vmlinuz-$(uname -r)}"
MEMORY="${MEMORY:-2G}"
CPUS="${CPUS:-2}"

build_initramfs() {
    echo "==> Building initramfs..."
    podman build --no-cache -t util-mdl-build .
    podman create --name util-mdl-export util-mdl-build
    podman cp util-mdl-export:/"$INITRAMFS" "$INITRAMFS"
    podman rm util-mdl-export
    echo "==> Initramfs written to $INITRAMFS"
}

usage() {
    echo "Usage: $0 [--build] [--test] [--kernel <path>]"
    echo ""
    echo "Options:"
    echo "  --build          Build the initramfs from Containerfile before launching"
    echo "  --test           Run testman integration tests instead of interactive QEMU"
    echo "  --kernel <path>  Path to kernel image (default: host kernel)"
    echo ""
    echo "Environment variables:"
    echo "  KERNEL   Kernel image path (default: /boot/vmlinuz-\$(uname -r))"
    echo "  MEMORY   VM memory (default: 2G)"
    echo "  CPUS     VM CPU count (default: 2)"
    echo "  KVM      Enable KVM acceleration (default: 1, set to 0 for CI)"
}

BUILD=0
TEST=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --build) BUILD=1; shift ;;
        --test) TEST=1; shift ;;
        --kernel) KERNEL="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown option: $1"; usage; exit 1 ;;
    esac
done

if [[ "$BUILD" -eq 1 ]]; then
    build_initramfs
fi

if [[ ! -f "$INITRAMFS" ]]; then
    echo "Error: $INITRAMFS not found. Run with --build to build it first."
    exit 1
fi

if [[ ! -f "$KERNEL" ]]; then
    echo "Error: kernel not found at $KERNEL"
    echo "Set KERNEL env var or pass --kernel <path>"
    exit 1
fi

if [[ "$TEST" -eq 1 ]]; then
    echo "==> Running testman integration tests"
    echo "    Kernel:    $KERNEL"
    echo "    Initramfs: $INITRAMFS"
    echo "    Memory:    $MEMORY"
    echo "    CPUs:      $CPUS"
    echo ""
    exec env \
        KERNEL="$KERNEL" \
        INITRAMFS="$INITRAMFS" \
        MEMORY="$MEMORY" \
        CPUS="$CPUS" \
        cargo run -p testman
fi

echo "==> Launching initramfs"
echo "    Kernel:    $KERNEL"
echo "    Initramfs: $INITRAMFS"
echo "    Memory:    $MEMORY"
echo "    CPUs:      $CPUS"
echo ""

exec qemu-system-x86_64 \
    -kernel "$KERNEL" \
    -initrd "$INITRAMFS" \
    -append "console=ttyS0 earlyprintk=ttyS0" \
    -nographic \
    -m "$MEMORY" \
    -smp "$CPUS" \
    -enable-kvm
