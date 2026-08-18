fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Only the server side: `actias test` fakes the kv service in-process
    // behind the same grpc surface, and worker-core already owns the client.
    tonic_build::configure()
        .build_client(false)
        .compile_protos(&["../protobufs/kv_service.proto"], &["../protobufs"])
        .unwrap();

    Ok(())
}
