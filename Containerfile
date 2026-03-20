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
RUN tar -xpvf nerdctl* -C out/
RUN cd out/bin && /out/bin/busybox --install -s ./

# init + package manager
FROM rust:alpine as util
RUN apk add git
RUN git clone https://gitlab.com/mtos-v2/util-mdl /mdl
RUN rustup default nightly
RUN rustup target add x86_64-unknown-linux-musl
RUN cd /mdl && cargo build --release --target x86_64-unknown-linux-musl

# packaging
FROM alpine:latest as stage1
COPY --from=stage0 out out
COPY --from=util /mdl/target/x86_64-unknown-linux-musl/release/actman out/bin/init
COPY --from=util /mdl/target/x86_64-unknown-linux-musl/release/updman out/bin/updman
RUN cd out && ln -sf bin sbin
RUN cd out && ln -sf bin/udhcpc etc/init/start/udhcpc
RUN cd out && ln -sf bin/buildkitd etc/init/start/buildkitd
RUN cd out && ln -sf bin/containerd etc/init/start/containerd
RUN cd out && ln -sf bin/sh etc/init/start/sh
RUN cd out && ln -sf bin/nerdctl bin/docker
RUN cd out && ln -sf bin/nerdctl bin/podman
RUN cd out && ln -sf bin/init bin/poweroff
RUN cd out && ln -sf bin/init bin/reboot
RUN cd out && ln -sf bin/init init
RUN cd out && find . | cpio -o -H newc | gzip > ../os.tar.gz

FROM scratch
COPY --from=stage1 os.tar.gz os.initramfs.tar.gz
