FROM rust:1.97 as builder
WORKDIR /app
COPY . .

# Move flat files into correct directory structure
RUN mkdir -p src static && \
    mv auth.rs config.rs db.rs main.rs models.rs routes.rs src/ && \
    mv index.html static/

RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/turso-service .
COPY --from=builder /app/static/ ./static/
EXPOSE 10000
CMD ["./turso-service"]
