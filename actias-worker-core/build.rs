fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(false)
        .compile_protos(
            &[
                "../protobufs/script_service.proto",
                "../protobufs/shared/bundle.proto",
                "../protobufs/kv_service.proto",
                "../protobufs/node_registry.proto",
                "../protobufs/secret_service.proto",
            ],
            &["../protobufs"],
        )
        .unwrap();

    // The worker both serves the data plane and calls its peers over it,
    // so this one proto builds both halves.
    tonic_build::configure()
        .compile_protos(&["../protobufs/worker_data.proto"], &["../protobufs"])
        .unwrap();

    Ok(())
}
