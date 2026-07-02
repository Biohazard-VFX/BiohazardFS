# syntax=docker/dockerfile:1

FROM rust:1.91-slim AS builder
WORKDIR /src
COPY . .
RUN cargo build --release -p biohazardfs-server

FROM debian:bookworm-slim AS runtime
RUN useradd --system --create-home --uid 10001 biohazardfs
COPY --from=builder /src/target/release/biohazardfs-server /usr/local/bin/biohazardfs-server
USER biohazardfs
ENTRYPOINT ["/usr/local/bin/biohazardfs-server"]
CMD ["serve", "--addr", "0.0.0.0:8080"]
