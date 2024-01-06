FROM rust:1.75-bookworm AS builder

COPY Cargo.toml Cargo.lock /usr/src/app/
COPY src /usr/src/app/src

WORKDIR /usr/src/app
RUN cargo build --release


FROM debian:bookworm-slim

# Install tini
ENV TINI_VERSION v0.19.0
ADD https://github.com/krallin/tini/releases/download/${TINI_VERSION}/tini /tini
RUN chmod +x /tini

# Copy app
COPY --from=builder /usr/src/app/target/release/prometheus_ping /usr/local/bin/prometheus_ping

# Set up user
RUN mkdir -p /usr/src/app/home && \
    useradd -d /usr/src/app/home -s /usr/sbin/nologin -u 998 appuser && \
    chown appuser /usr/src/app/home

USER 998
ENTRYPOINT ["/tini", "--", "prometheus_ping"]
