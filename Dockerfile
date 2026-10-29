# syntax=docker/dockerfile:1

# All build stages run on the build machine and cross-compile for the target platform, so
# multi-arch images need no emulation, which would make the DuckDB C++ build take hours.

# Each compile job of the bundled DuckDB C++ sources needs up to about 1 GB of memory, so
# unlimited parallelism exhausts the memory of typical Docker VMs.
ARG BUILD_JOBS=4

# Trixie, because the prebuilt dioxus-cli needs glibc 2.39; the stage only produces static files.
FROM --platform=$BUILDPLATFORM rust:1-trixie AS dashboard
ARG BUILD_JOBS
ENV CARGO_BUILD_JOBS=$BUILD_JOBS
# The prebuilt release binary, pinned by checksum; building it from source takes longer than
# the rest of the image.
ARG DIOXUS_CLI_VERSION=0.7.10
ARG DIOXUS_CLI_SHA256_AARCH64=8f1a17d3218700ffbe15e6540d936a178b2556fc801121a31082e3ba4ab9ef55
ARG DIOXUS_CLI_SHA256_X86_64=4363e4ed2a3f1eb7f4d38d2d59aed59ce43271c44c16b425e92c89a64761fbe7
RUN rustup target add wasm32-unknown-unknown
RUN arch="$(uname -m)" \
    && case "$arch" in \
        aarch64) sha256="$DIOXUS_CLI_SHA256_AARCH64" ;; \
        x86_64) sha256="$DIOXUS_CLI_SHA256_X86_64" ;; \
        *) echo "no dioxus-cli release for $arch" >&2; exit 1 ;; \
    esac \
    && archive="dx-$arch-unknown-linux-gnu.tar.gz" \
    && curl -fsSL -o "/tmp/$archive" \
        "https://github.com/DioxusLabs/dioxus/releases/download/v$DIOXUS_CLI_VERSION/$archive" \
    && echo "$sha256  /tmp/$archive" | sha256sum -c - \
    && tar -xzf "/tmp/$archive" -C /usr/local/cargo/bin dx \
    && rm "/tmp/$archive" \
    && dx --version
WORKDIR /src
COPY . .
# Locked: the dashboard and per-architecture server builds run in parallel and would
# otherwise unpack crates over each other.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,id=dashboard-target \
    dx build --release --web -p dashboard --debug-symbols false \
    && cp -r target/dx/dashboard/release/web/public /dashboard

FROM --platform=$BUILDPLATFORM rust:1-bookworm AS server
ARG TARGETARCH
ARG BUILD_JOBS
ENV CARGO_BUILD_JOBS=$BUILD_JOBS
# Debian names its compilers by target triple, natively as well as for cross-compiling.
ENV CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++ \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
    CXX_x86_64_unknown_linux_gnu=x86_64-linux-gnu-g++ \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc
RUN case "$TARGETARCH" in \
        arm64) triple=aarch64-unknown-linux-gnu; cross="g++-aarch64-linux-gnu libc6-dev-arm64-cross" ;; \
        amd64) triple=x86_64-unknown-linux-gnu; cross="g++-x86-64-linux-gnu libc6-dev-amd64-cross" ;; \
        *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac \
    && if [ "$TARGETARCH" = "$(dpkg --print-architecture)" ]; then cross=""; fi \
    && apt-get update && apt-get install -y --no-install-recommends libcap2-bin $cross \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add "$triple" \
    && echo "$triple" > /target-triple
WORKDIR /src
COPY . .
# Locked: the dashboard and per-architecture server builds run in parallel and would
# otherwise unpack crates over each other.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,id=server-target-$TARGETARCH \
    triple="$(cat /target-triple)" \
    && cargo build --release --locked -p server --target "$triple" \
    && cp "target/$triple/release/server" /server \
    && mkdir -p /data/blocklists
# Lets the nonroot user bind port 53 with host networking, where Docker does not lower the
# privileged port range as it does in a container's own network namespace.
RUN setcap cap_net_bind_service=+ep /server

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=server /server /usr/local/bin/server
COPY --from=dashboard /dashboard /usr/share/dns/dashboard
# Owned by the nonroot user, so named volumes mounted here start out writable.
COPY --from=server --chown=nonroot:nonroot /data /data
ENV DNS_DASHBOARD_DIR=/usr/share/dns/dashboard
# glibc raises its mmap threshold after the first large free, so the multi-MB buffers of
# later blocklist rebuilds stay on the heap and are never returned. A fixed threshold keeps
# memory flat across refreshes (measured: 164 MB → ~87 MB after several refreshes).
ENV MALLOC_MMAP_THRESHOLD_=1048576
VOLUME /data
EXPOSE 53/udp 53/tcp 8053/tcp
ENTRYPOINT ["/usr/local/bin/server"]
