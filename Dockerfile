FROM rustlang/rust:nightly

USER 1001
COPY . .
RUN touch /storage/test.sqlite
RUN cargo install sqlx-cli
RUN sqlx migrate run

RUN cargo build --releaso


ENTRYPOINT ["./target/release/picvoter-backend"]