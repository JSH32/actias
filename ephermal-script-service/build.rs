fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_client(false)
        .compile(
            &[
                "../protobufs/shared/bundle.proto",
                "../protobufs/script_service.proto",
            ],
            &["../protobufs"],
        )
        .unwrap();
    Ok(())
}
