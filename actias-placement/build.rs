fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_client(false)
        .compile_protos(&["../protobufs/node_registry.proto"], &["../protobufs"])?;
    Ok(())
}
