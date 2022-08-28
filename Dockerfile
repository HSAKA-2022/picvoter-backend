FROM rustlang/rust:nightly

RUN mkdir -p /app/storage
WORKDIR /app
COPY . .
RUN cargo install sqlx-cli
RUN sqlx migrate run

RUN cargo build --releaso

ENTRYPOINT ["./target/release/picvoter-backend"]