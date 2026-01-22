FROM rust:1.91.1 AS builder


WORKDIR /usr/src/mijn_bussie

# COPY vendor ./vendor
# COPY docker_cargo/config.toml ./.cargo/config.toml

COPY ./src ./src
COPY Cargo.lock ./
COPY Cargo.toml ./
COPY entity ./entity
COPY migration ./migration

RUN cargo build --release

# ---- Final Stage ----
FROM ubuntu:24.04

WORKDIR /app

# Copy binary only
COPY --from=builder /usr/src/mijn_bussie/target/release/mijn_bussie .

COPY ./templates /app/templates

RUN apt-get update && apt-get install -y openssl && rm -rf /var/lib/apt/lists/*

RUN mkdir cert
RUN openssl req -new -newkey rsa:4096 -x509 -sha256 -nodes -out cert/cert.crt -keyout cert/key.key -subj "/C=NL/ST=NB/L=EHV/CN=mijn_bussie"

RUN ls -lah ./
RUN pwd
RUN ldd ./mijn_bussie

CMD ["/app/mijn_bussie"]