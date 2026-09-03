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
    // The replication payloads are `Bytes`, so a flight's frames are
    // sliced per replica, never copied.
    tonic_build::configure()
        .bytes([
            "WalAppend.bytes",
            "GenerationChunk.bytes",
            "ReplicaChunk.base",
            "ReplicaChunk.wal",
        ])
        .compile_protos(&["../protobufs/worker_data.proto"], &["../protobufs"])
        .unwrap();

    Ok(())
}
