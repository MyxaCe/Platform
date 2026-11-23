# Зависимости собираются отдельным слоем (cargo-chef): правка нашего кода
# не пересобирает крейты из Cargo.lock, слой берётся из кеша сборки.
FROM rust:slim AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release -p gateway --recipe-path recipe.json
COPY . .
RUN cargo build --release -p gateway

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/gateway /usr/local/bin/gateway
EXPOSE 8080
CMD ["gateway"]
