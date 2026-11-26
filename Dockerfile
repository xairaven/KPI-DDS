# One Dockerfile for all three BMS services.
# The builder compiles them together, then each final stage takes one binary:
#
#   docker build --target bms-ingest        -t bms-ingest .
#   docker build --target bms-state         -t bms-state .
#   docker build --target bms-deconfliction -t bms-deconfliction .
#
# BuildKit builds the shared builder stage once and reuses it for every target.

# ---------- build ----------
FROM rust:1.100-slim-trixie AS builder
WORKDIR /src

COPY Cargo.toml Cargo.lock clippy.toml ./
COPY crates ./crates

# Cache mounts keep downloaded crates and compiled dependencies between builds,
# so changing one line of our code does not recompile all of tokio and sqlx.
# The target directory is a cache, not part of the image, so the binaries are
# copied out of it in the same step.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
        -p bms-ingest -p bms-state -p bms-deconfliction \
 && mkdir /out \
 && cp target/release/bms-ingest target/release/bms-state target/release/bms-deconfliction /out/

# ---------- runtime ----------
# Distroless: glibc and CA certificates, but no shell or package manager.
# Less software in the image means fewer vulnerabilities; "nonroot" runs as UID 65532.
FROM gcr.io/distroless/cc-debian13:nonroot AS runtime
ENV RUST_LOG=info
# Containers have no terminal, so plain logs without colour codes.
ENV NO_COLOR=1

FROM runtime AS bms-ingest
COPY --from=builder /out/bms-ingest /usr/local/bin/
ENTRYPOINT ["bms-ingest"]

FROM runtime AS bms-deconfliction
COPY --from=builder /out/bms-deconfliction /usr/local/bin/
ENTRYPOINT ["bms-deconfliction"]

# Last stage = the default target of a plain "docker build".
FROM runtime AS bms-state
COPY --from=builder /out/bms-state /usr/local/bin/
EXPOSE 9000
ENTRYPOINT ["bms-state"]
