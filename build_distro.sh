#!/bin/bash
set -Eeux -o pipefail

if [[ $(id -u) != 0 ]]; then exit 1; fi

base_url=https://tux.rainside.sk/alpine/latest-stable/releases/$(uname -m)
alpine_file=$(curl https://tux.rainside.sk/alpine/latest-stable/releases/$(uname -m)/latest-releases.yaml | grep file | head -n3 | tail -n1 | awk '{print $2}')

curl -LO --progress-bar $base_url/$alpine_file

mkdir -p out/{dev,run,sys,proc,tmp,home,bin,etc/init/start,etc/init/stop} dl

tar -xpvf $alpine_file -C dl

cp --dereference /etc/resolv.conf dl/etc

arch-chroot dl apk add busybox-static

cp dl/bin/busybox.static out/bin/busybox

cd out/bin

busybox --install -s ./

[[ ! -d util-mdl ]]; git clone https://gitlab.com/mtos-v2/util-mdl && cd util-mdl

cargo build --release --target $(uname -m)-unknown-linux-musl

cp target/release/{act,upd}man out/bin || cp target/release/{act,upd}man ../out/bin

