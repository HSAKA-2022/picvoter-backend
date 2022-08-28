FROM rustlang/rust:nightly

USER 1001
COPY . .
RUN cargo install sqlx-cli
RUN sqlx migrate run

RUN cargo build --releaso


ENTRYPOINT ["./target/release/picvoter-backend"]