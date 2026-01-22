FROM rust:1.91.1 AS builder

WORKDIR /usr/src/mijn_bussie_auth
COPY ./src ./src
COPY Cargo.lock ./
COPY Cargo.toml ./
COPY entity ./entity

RUN cargo build --release

# ---- Final Stage ----
FROM ubuntu:24.04

WORKDIR /app

# Copy binary only
COPY --from=builder /usr/src/mijn_bussie_auth/target/release/mijn_bussie_auth .

RUN apt-get update && apt-get install -y openssl && rm -rf /var/lib/apt/lists/*

RUN mkdir cert
RUN openssl req -new -newkey rsa:4096 -x509 -sha256 -nodes -out cert/cert.crt -keyout cert/key.key -subj "/C=NL/ST=NB/L=EHV/CN=mijn_bussie_auth"

CMD ["/app/mijn_bussie_auth"]