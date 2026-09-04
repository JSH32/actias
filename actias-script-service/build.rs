fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_client(true)
        .type_attribute(
            "bundle.File",
            "#[derive(serde::Serialize, serde::Deserialize)]",
        )
        .type_attribute(
            "script_service.ScriptConfig",
            "#[derive(serde::Serialize, serde::Deserialize)]",
        )
        .type_attribute(
            "script_service.Capabilities",
            "#[derive(serde::Serialize, serde::Deserialize)]",
        )
        .type_attribute(
            "bundle.Bundle",
            "#[derive(serde::Serialize, serde::Deserialize)]",
        )
        .compile_protos(
            &[
                "../protobufs/shared/bundle.proto",
                "../protobufs/script_service.proto",
                "../protobufs/node_registry.proto",
            ],
            &["../protobufs"],
        )
        .unwrap();
    Ok(())
}
