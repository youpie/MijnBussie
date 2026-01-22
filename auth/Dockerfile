FROM rust:1.91.1 AS builder

WORKDIR /usr/src/mijn_bussie_auth
COPY ./src ./src
COPY Cargo.lock ./
COPY Cargo.toml ./
COPY entity ./entity

RUN cargo build --release

# ---- Final Stage ----
FROM alpine:3.23

WORKDIR /app

# Copy binary only
COPY --from=builder /usr/src/mijn_bussie_auth/target/release/mijn_bussie_auth .

RUN apk add --no-cache openssl

RUN mkdir cert
RUN openssl req -new -newkey rsa:4096 -x509 -sha256 -nodes -out cert/cert.crt -keyout cert/key.key -subj "/C=NL/ST=NB/L=EHV/CN=mijn_bussie_auth"

CMD ["./mijn_bussie_auth"]