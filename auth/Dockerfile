FROM rust:1.91.1 AS builder

WORKDIR /usr/src/mijn_bussie_auth

# Copy vendor files to reduce pointless bandwidth use
COPY vendor ./vendor
COPY docker_cargo/config.toml ./.cargo/config.toml

COPY Cargo.lock ./
COPY Cargo.toml ./
COPY entity ./entity

# Create an empty file, when cargo compiles the empty file it will also compile all dependencies.
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release

# If we then remove this file and copy the actual files, it will cache the first build. And only compile the main source again, unless you change cargo.toml
RUN rm -rf src
COPY src ./src
RUN touch src/main.rs
# RUN ls -lah target/release/

RUN cargo build --release -p mijn_bussie_auth

# ---- Final Stage ----
FROM ubuntu:24.04

WORKDIR /app

# Copy binary only
COPY --from=builder /usr/src/mijn_bussie_auth/target/release/mijn_bussie_auth .

RUN apt-get update && apt-get install -y openssl ca-certificates && rm -rf /var/lib/apt/lists/*

RUN mkdir cert
RUN openssl req -new -newkey rsa:4096 -x509 -sha256 -nodes -out cert/cert.crt -keyout cert/key.key -subj "/C=NL/ST=NB/L=EHV/CN=mijn_bussie_auth"

CMD ["/app/mijn_bussie_auth"]
