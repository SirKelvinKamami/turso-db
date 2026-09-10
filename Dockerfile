FROM rust:1.97 as builder
RUN apt-get update && apt-get install -y gcc g++ make cmake pkg-config && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY . .
RUN cargo build --release

# Runtime must match (or exceed) the glibc the builder image links against: newer
# rust images build against a newer glibc (>= 2.38), which bookworm-slim (2.36) can't
# load. trixie-slim ships glibc 2.41.
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
RUN mkdir -p /data
WORKDIR /app
COPY --from=builder /app/target/release/turso-service .
COPY --from=builder /app/static/ ./static/
EXPOSE 10000
CMD ["./turso-service"]
