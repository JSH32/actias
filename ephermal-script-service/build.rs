fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = prost_build::Config::new();
    config.protoc_arg("--experimental_allow_proto3_optional");

    tonic_build::configure()
        .build_client(false)
        .type_attribute(
            "script_service.Script",
            "#[derive(serde::Serialize, serde::Deserialize)]",
        )
        .type_attribute(
            "script_service.Bundle",
            "#[derive(serde::Serialize, serde::Deserialize)]",
        )
        .type_attribute(
            "script_service.File",
            "#[derive(serde::Serialize, serde::Deserialize)]",
        )
        .compile_with_config(
            config,
            &["../protobufs/script_service.proto"],
            &["../protobufs"],
        )
        .unwrap();
    Ok(())
}
