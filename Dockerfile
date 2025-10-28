FROM rust:1.89.0

WORKDIR /usr/src/mijn_bussie
COPY ./src ./src
COPY Cargo.lock ./
COPY Cargo.toml ./

COPY ./templates /usr/src/mijn_bussie/templates

RUN cargo install --path .