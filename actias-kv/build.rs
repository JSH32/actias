fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_client(false)
        .compile(&["../protobufs/kv_service.proto"], &["../protobufs"])
        .unwrap();
    Ok(())
}
