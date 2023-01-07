FROM rust:1.60 as build

# Create a new empty shell project
RUN USER=root cargo new --bin ephermal-script-service
WORKDIR /ephermal-script-service

# Copy source
COPY ./Cargo.toml ./Cargo.toml
COPY ./Cargo.lock ./Cargo.lock
COPY ./src ./src
COPY ./.git ./.git

# Build release
RUN cargo build --release


# Running stage
FROM gcr.io/distroless/cc

# Copy the build artifact from the build stage
COPY --from=build /ephermal-script-service/target/release/ephermal-script-service .

CMD ["./ephermal-script-service"]