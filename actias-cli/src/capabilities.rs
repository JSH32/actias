//! Local pre-flight of the declaration pass: the same extraction the
//! platform runs authoritatively at publish, run here first so syntax
//! errors and the derived contract surface before anything uploads.

use std::collections::HashMap;

use base64::Engine;

use crate::script::ScriptConfig;

pub use actias_declarations::Declarations;

/// Runs the declaration pass over the project's bundle.
///
/// # Errors
/// Returns text describing the failure: a file that does not glob, a syntax
/// error, or a runtime error in top-level code.
pub fn extract(config: &ScriptConfig) -> Result<Declarations, String> {
    let bundle = config.to_bundle()?;

    let mut files: HashMap<String, String> = HashMap::new();
    for file in &bundle.files {
        let content = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(&file.content)
            .map_err(|e| format!("{}: {e}", file.file_path))?;

        if let Ok(source) = String::from_utf8(content) {
            files.insert(file.file_path.clone(), source);
        }
    }

    actias_declarations::extract(files, &config.entry_point)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// The glob-and-decode plumbing over a real directory; the pass itself
    /// is covered where it lives, in actias-declarations.
    #[test]
    fn a_project_directory_extracts_through_the_shared_pass() {
        let dir = tempfile::tempdir().expect("tempdir");

        let mut file = std::fs::File::create(dir.path().join("main.lua")).expect("file");
        file.write_all(br#"local visits = kv "visits" on "fetch" (function() end)"#)
            .expect("write");

        let config: ScriptConfig = serde_json::from_str(
            r#"{"id":"00000000-0000-0000-0000-000000000000",
                "entryPoint":"main.lua","includes":["**/*.lua"],"ignore":[]}"#,
        )
        .expect("config parses");
        let mut config = config;
        config.project_path = Some(dir.path().to_path_buf());

        let declarations = extract(&config).expect("extraction succeeds");
        assert_eq!(declarations.kv, vec!["visits"]);
        assert_eq!(declarations.events, vec!["fetch"]);
    }
}
