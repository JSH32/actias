fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = prost_build::Config::new();
    config.protoc_arg("--experimental_allow_proto3_optional");

    tonic_build::configure()
        .build_server(false)
        .compile_with_config(
            config,
            &[
                "../protobufs/script_service.proto",
                "../protobufs/shared/bundle.proto",
            ],
            &["../"],
        )
        .unwrap();

    Ok(())
}
