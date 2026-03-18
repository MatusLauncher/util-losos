# check=skip=FromAsCasing
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
RUN cd out/bin
RUN /out/bin/busybox --install -s ./
RUN apk add curl tar
RUN curl -LO $(curl -s https://api.github.com/repos/containerd/nerdctl/releases/latest | grep full | grep browser_download_url | head -n1 | awk '{print $2}' | cut -d '"' -f2)
RUN tar -xpvf nerdctl* -C out/

FROM rust:alpine as util
RUN apk add git
RUN git clone https://gitlab.com/mtos-v2/util-mdl /mdl
RUN rustup default nightly
RUN rustup target add x86_64-unknown-linux-musl
RUN cd /mdl && cargo build --release --target x86_64-unknown-linux-musl

FROM alpine:latest as stage1
COPY --from=stage0 out out
COPY --from=util /mdl/target/x86_64-unknown-linux-musl/release/actman out/bin/init
COPY --from=util /mdl/target/release/x86_64-unknown-linux-musl/release/updman out/bin/updman
RUN ln -sf out/bin out/sbin
RUN ln -sf out/bin/udhcpc out/etc/init/start/udhcpc
RUN ln -sf out/bin/buildkitd out/etc/init/start/buildkitd
RUN ln -sf out/bin/nerdctl out/bin/docker
RUN ln -sf out/bin/nerdctl out/bin/podman
RUN tar -czvf os.tar.gz out/*

FROM scratch
COPY --from=stage1 os.tar.gz os.initramfs.tar.gz
