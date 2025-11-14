FROM rust:1.91.1

WORKDIR /usr/src/mijn_bussie
COPY ./src ./src
COPY Cargo.lock ./
COPY Cargo.toml ./
COPY entity ./entity
COPY migration ./migration

COPY ./templates /usr/src/mijn_bussie/templates

RUN mkdir cert
RUN openssl req -new -newkey rsa:4096 -x509 -sha256 -nodes -out cert/cert.crt -keyout cert/key.key -subj "/C=NL/ST=NB/L=EHV/CN=mijn_bussie"

RUN cargo install --path .

CMD ["mijn_bussie"]