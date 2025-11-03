FROM rust:1.89.0

WORKDIR /usr/src/mijn_bussie_auth
COPY ./src ./src
COPY Cargo.lock ./
COPY Cargo.toml ./

RUN cargo install --path .

CMD ["mijn_bussie_auth"]