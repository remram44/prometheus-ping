FROM rust:1.97-trixie AS builder

COPY Cargo.toml Cargo.lock /usr/src/app/
COPY src /usr/src/app/src

WORKDIR /usr/src/app
RUN cargo build --release


FROM --platform=$BUILDPLATFORM rust:1.97-trixie AS tini-getter

ARG TARGETPLATFORM

ENV TINI_VERSION=v0.19.0
RUN if [ ${TARGETPLATFORM} = "linux/amd64" ]; then TINI_NAME=tini-amd64; \
    elif [ ${TARGETPLATFORM} = "linux/arm64" ]; then TINI_NAME=tini-arm64; \
    else echo "no tini URL for ${TARGETPLATFORM}"; exit 1; fi && \
    curl -sSLfo /tini https://github.com/krallin/tini/releases/download/${TINI_VERSION}/${TINI_NAME} && \
    chmod +x /tini


FROM debian:trixie-slim

# Install tini
COPY --from=tini-getter /tini /tini

# Copy app
COPY --from=builder /usr/src/app/target/release/prometheus_ping /usr/local/bin/prometheus_ping

# Set up user
RUN mkdir -p /usr/src/app/home && \
    useradd -d /usr/src/app/home -s /usr/sbin/nologin -u 998 appuser && \
    chown appuser /usr/src/app/home

USER 998
ENTRYPOINT ["/tini", "--", "prometheus_ping"]
