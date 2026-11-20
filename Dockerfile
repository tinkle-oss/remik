FROM rust:1.100.0-slim-bookworm@sha256:7c7d4eb4d05b7781187a327809466cfddf29753f6defdc3d19a0e291951b7a37 AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY benches ./benches
RUN cargo build --locked --release --bin remik

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc
ARG VERSION=dev
ARG REVISION=unknown
LABEL org.opencontainers.image.source="https://github.com/tinkle-oss/remik" \
      org.opencontainers.image.title="Remik" \
      org.opencontainers.image.description="Memory-only RESP2 string server; no persistence or TLS" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}"
COPY --from=build /build/target/release/remik /usr/local/bin/remik
COPY LICENSE /usr/share/licenses/remik/LICENSE
COPY target/THIRD_PARTY_NOTICES.txt /usr/share/licenses/remik/THIRD_PARTY_NOTICES.txt
USER 10001:10001
ENV REMIK_BIND=0.0.0.0:6379
EXPOSE 6379
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 CMD ["/usr/local/bin/remik", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/remik"]
