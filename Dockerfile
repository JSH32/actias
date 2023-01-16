FROM rust:1.66 as chef
RUN cargo install cargo-chef 

WORKDIR /app

FROM chef AS planner
# Copy entire workspace
COPY ./Cargo.toml ./Cargo.toml
COPY ./Cargo.lock ./Cargo.lock
COPY ./protobufs ./protobufs
COPY ./actias-common ./actias-common
COPY ./actias-script-service ./actias-script-service
COPY ./actias-worker ./actias-worker
RUN cargo chef prepare  --recipe-path recipe.json

FROM chef AS builder

# Install protobuf compiler.
RUN apt update && apt install -y protobuf-compiler

COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
ARG CRATE_NAME

COPY . .
RUN cargo build --release --bin ${CRATE_NAME}

FROM gcr.io/distroless/cc as runtime
ARG CRATE_NAME

WORKDIR /app
COPY --from=builder /app/target/release/${CRATE_NAME} /app
CMD [ "./${CRATE_NAME}" ]

LABEL org.opencontainers.image.source="https://github.com/jsh32/actias"