FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential ca-certificates curl file git libayatana-appindicator3-dev \
    libfuse2 libssl-dev libwebkit2gtk-4.1-dev librsvg2-dev patchelf pkg-config xz-utils \
    && rm -rf /var/lib/apt/lists/*

ARG NODE_VERSION=22.18.0
ARG RUST_VERSION=1.93.1
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo
ENV PATH=/opt/cargo/bin:/usr/local/bin:/usr/bin:/bin
RUN curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz" \
    | tar -xJ --strip-components=1 -C /usr/local
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain "${RUST_VERSION}"

WORKDIR /work
