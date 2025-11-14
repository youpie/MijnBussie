FROM rust:1.91.1

WORKDIR /usr/src/mijn_bussie_auth
COPY ./src ./src
COPY Cargo.lock ./
COPY Cargo.toml ./
COPY entity ./entity

RUN cargo install --path .

RUN mkdir cert
RUN openssl req -new -newkey rsa:4096 -x509 -sha256 -nodes -out cert/cert.crt -keyout cert/key.key -subj "/C=NL/ST=NB/L=EHV/CN=mijn_bussie_auth"

CMD ["mijn_bussie_auth"]