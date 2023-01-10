fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(false)
        .type_attribute(
            "script_service.Script",
            "#[derive(serde::Serialize, serde::Deserialize)]",
        )
        .type_attribute(
            "bundle.Bundle",
            "#[derive(serde::Serialize, serde::Deserialize)]",
        )
        .type_attribute(
            "bundle.File",
            "#[derive(serde::Serialize, serde::Deserialize)]",
        )
        .type_attribute(
            "script_service.Revision",
            "#[derive(serde::Serialize, serde::Deserialize)]",
        )
        .compile(
            &[
                "../protobufs/script_service.proto",
                "../protobufs/shared/bundle.proto",
            ],
            &["../protobufs"],
        )
        .unwrap();

    Ok(())
}
