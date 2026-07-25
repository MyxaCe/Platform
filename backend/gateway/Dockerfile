# Сборка бинаря шлюза (release), затем тонкий рантайм-образ.
FROM rust:slim AS build
WORKDIR /app
COPY . .
RUN cargo build --release -p gateway

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/gateway /usr/local/bin/gateway
EXPOSE 8080
CMD ["gateway"]
