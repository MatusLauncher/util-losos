# check=skip=FromAsCasing
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

# init + package manager
FROM rust:alpine as util
COPY . /mdl
RUN cd /mdl \
    && cargo build --release --target x86_64-unknown-linux-musl \
    && cp target/x86_64-unknown-linux-musl/release/actman /actman \
    && cp target/x86_64-unknown-linux-musl/release/updman /updman \
    && cp target/x86_64-unknown-linux-musl/release/dhcman /dhcman \
    && cp target/x86_64-unknown-linux-musl/release/cluman /cluman \
    && rm -rf target /root/.cargo/registry

# packaging
FROM alpine:latest as stage1
ARG MODE
COPY --from=stage0 out out
COPY --from=util /actman out/bin/init
COPY --from=util /updman out/bin/updman
COPY --from=util /dhcman out/bin/dhcman
COPY --from=util /cluman out/bin/cluman
RUN cd out && ln -sf bin sbin
RUN printf '#!/bin/sh\nip link set lo up && ip addr add 127.0.0.1/8 dev lo\n' > out/etc/init/start/00-loopback \
    && chmod +x out/etc/init/start/00-loopback
RUN cd out && ln -sf bin/dhcman etc/init/start/00-eth0
RUN cd out && ln -sf bin/buildkitd etc/init/start/buildkitd
RUN cd out && ln -sf bin/containerd etc/init/start/containerd
RUN cd out && ln -sf bin/sh etc/init/start/sh
RUN cd out && ln -sf bin/nerdctl bin/docker
RUN cd out && ln -sf bin/nerdctl bin/podman
RUN cd out && ln -sf bin/init bin/poweroff
RUN cd out && ln -sf bin/init bin/reboot
RUN cd out && ln -sf bin/init init
# cluman mode — the binary dispatches on argv[0], so symlink it to the mode name.
# client/server are boot-time daemons started by init; controller is a one-shot
# CLI tool and is only installed as a named symlink without an init entry.
RUN cd out && ln -sf  
RUN apk add fakeroot
RUN fakeroot sh -c 'mknod out/dev/console c 5 1 && cd out && find . | cpio -o -H newc | gzip > ../os.tar.gz'
# final
FROM scratch
COPY --from=stage1 os.tar.gz os.initramfs.tar.gz
