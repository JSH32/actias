fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(false)
        .compile(
            &[
                "../protobufs/script_service.proto",
                "../protobufs/shared/bundle.proto",
                "../protobufs/kv_service.proto",
            ],
            &["../protobufs"],
        )
        .unwrap();

    Ok(())
}
