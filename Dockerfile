FROM rust:1.91.1 AS builder

WORKDIR /usr/src/mijn_bussie
COPY ./src ./src
COPY Cargo.lock ./
COPY Cargo.toml ./
COPY entity ./entity
COPY migration ./migration

RUN cargo build --release

# ---- Final Stage ----
FROM alpine:3.23

WORKDIR /app

# Copy binary only
COPY --from=builder /usr/src/mijn_bussie/target/release/mijn_bussie .

COPY ./templates /app/templates

RUN apk add --no-cache openssl

RUN mkdir cert
RUN openssl req -new -newkey rsa:4096 -x509 -sha256 -nodes -out cert/cert.crt -keyout cert/key.key -subj "/C=NL/ST=NB/L=EHV/CN=mijn_bussie"

CMD ["./mijn_bussie"]