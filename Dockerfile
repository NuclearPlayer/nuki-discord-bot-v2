FROM rust:1-trixie AS build
WORKDIR /app
COPY . .
RUN cargo build --release --locked

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/nuki /usr/local/bin/nuki
CMD ["nuki"]
