fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Only the server side: `actias test` fakes the kv and secret services
    // in-process behind the same grpc surfaces, and worker-core already
    // owns the clients.
    tonic_build::configure()
        .build_client(false)
        .compile_protos(
            &[
                "../protobufs/kv_service.proto",
                "../protobufs/secret_service.proto",
            ],
            &["../protobufs"],
        )
        .unwrap();

    Ok(())
}
