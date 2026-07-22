# Сборка бинаря шлюза (release), затем тонкий рантайм-образ.
FROM rust:slim AS build
WORKDIR /app
# webauthn-rs (ADR-020) линкуется с системной OpenSSL — нужны заголовки и pkg-config.
RUN apt-get update && apt-get install -y --no-install-recommends libssl-dev pkg-config && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --release -p gateway

FROM debian:bookworm-slim
# Бинарь динамически линкует libssl — в рантайм-образ нужна сама библиотека и корневые сертификаты.
RUN apt-get update && apt-get install -y --no-install-recommends libssl3 ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/gateway /usr/local/bin/gateway
EXPOSE 8080
CMD ["gateway"]
