# Сборка бинаря шлюза (release), затем тонкий рантайм-образ.
FROM rust:slim AS build
WORKDIR /app
COPY . .
RUN cargo build --release -p gateway

FROM debian:bookworm-slim
COPY --from=build /app/target/release/gateway /usr/local/bin/gateway
EXPOSE 8080
CMD ["gateway"]
