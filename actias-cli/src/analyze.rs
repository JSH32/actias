//! Typed checking via luau-analyze: every bundle source is copied to a
//! shadow file that opens with typed locals shadowing the platform globals,
//! and the analyzer runs over those. Shadowing beats definition files
//! because the stock luau-analyze binary silently ignores its definitions
//! flags; reported line numbers are shifted back so they point at the
//! user's own code.
//!
//! Checking is gradual, luau's own model: nonstrict by default, which
//! catches unknown globals (typos against the platform surface) while
//! tolerating untyped lua; a file opening with `--!strict` gets full
//! checking, so its directive is hoisted above the prologue where luau
//! requires it to sit.

use std::process::Command;

use base64::Engine;
use colored::*;

use crate::script::ScriptConfig;

/// The platform's ambient Luau declarations, one file per domain.
pub const DEFINITION_FILES: [(&str, &str); 4] = [
    ("core.d.luau", include_str!("../definitions/core.d.luau")),
    (
        "objects.d.luau",
        include_str!("../definitions/objects.d.luau"),
    ),
    ("work.d.luau", include_str!("../definitions/work.d.luau")),
    ("http.d.luau", include_str!("../definitions/http.d.luau")),
];

/// Converts [`DEFINITION_FILES`] into typed local shadows: each
/// single-line `declare name: T` becomes `local name: T = nil :: any`,
/// other lines pass through, and a keep-alive expression suppresses
/// unused warnings. Shadows instead of `--defs` because luau-analyze
/// ignores definition-file flags.
fn prologue() -> String {
    let mut out = String::from("-- actias: typed shadows derived from the definitions files\n");
    let mut names: Vec<&str> = Vec::new();
    for line in DEFINITION_FILES
        .iter()
        .flat_map(|(_, content)| content.lines())
    {
        if let Some(rest) = line.strip_prefix("declare ") {
            let name = rest.split(':').next().unwrap_or("").trim();
            names.push(name);
            out.push_str("local ");
            out.push_str(rest);
            out.push_str(" = nil :: any\n");
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("local _ = ");
    out.push_str(&names.join(" and "));
    out.push('\n');
    out
}

/// Runs the strict type check over the project's bundle.
///
/// A missing analyzer is a warning, not a failure: the declaration pass has
/// already validated the project, and not every machine ships luau-analyze.
///
/// # Errors
/// Returns text when the analyzer reports type errors or cannot run.
pub fn analyze(config: &ScriptConfig) -> Result<(), String> {
    let bundle = config.to_bundle()?;

    let root = std::env::temp_dir().join(format!("actias-check-{}", uuid::Uuid::new_v4()));

    let mut targets: Vec<String> = Vec::new();
    for file in &bundle.files {
        if !file.file_path.ends_with(".lua") {
            continue;
        }

        let content = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(&file.content)
            .map_err(|e| format!("{}: {e}", file.file_path))?;
        let Ok(source) = String::from_utf8(content) else {
            continue;
        };

        let shadow = root.join(&file.file_path);
        if let Some(parent) = shadow.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&shadow, shadow_source(&source)).map_err(|e| e.to_string())?;

        targets.push(file.file_path.clone());
    }

    if targets.is_empty() {
        return Ok(());
    }

    let output = Command::new("luau-analyze")
        .args(&targets)
        .current_dir(&root)
        .output();
    let cleanup = || {
        let _ = std::fs::remove_dir_all(&root);
    };

    let output = match output {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            cleanup();
            println!(
                "{}",
                "⚠️ luau-analyze not found; skipping the type check.".yellow()
            );
            return Ok(());
        }
        Err(error) => {
            cleanup();
            return Err(format!("luau-analyze could not run: {error}"));
        }
    };
    cleanup();

    let offset = prologue().lines().count();
    for line in String::from_utf8_lossy(&output.stdout)
        .lines()
        .chain(String::from_utf8_lossy(&output.stderr).lines())
    {
        println!("{}", shift_line(line, offset));
    }

    if output.status.success() {
        Ok(())
    } else {
        Err("The type check found errors.".to_owned())
    }
}

/// One shadow file: the user's leading `--!` directives (luau only honors
/// them at the very top, so they hoist above the prologue), the prologue,
/// then the rest of the source.
fn shadow_source(source: &str) -> String {
    let directives: Vec<&str> = source
        .lines()
        .take_while(|line| line.trim_start().starts_with("--!"))
        .collect();

    let rest: String = source
        .lines()
        .skip(directives.len())
        .map(|line| format!("{line}\n"))
        .collect();

    let mut shadow = String::new();
    for directive in &directives {
        shadow.push_str(directive);
        shadow.push('\n');
    }
    shadow.push_str(&prologue());
    shadow.push_str(&rest);
    shadow
}

/// Rewrites one `./path(line,col): ...` diagnostic back to the user's line
/// numbers by removing the prologue's share; anything else passes through.
fn shift_line(line: &str, offset: usize) -> String {
    let Some(open) = line.find('(') else {
        return line.to_owned();
    };
    let Some(comma) = line[open + 1..].find(',') else {
        return line.to_owned();
    };
    let Ok(reported) = line[open + 1..open + 1 + comma].parse::<usize>() else {
        return line.to_owned();
    };

    format!(
        "{}({}{}",
        &line[..open],
        reported.saturating_sub(offset),
        &line[open + 1 + comma..]
    )
}

/// Where the given sources live as a checkable project on disk.
#[cfg(test)]
fn project(dir: &std::path::Path, files: &[(&str, &str)]) -> ScriptConfig {
    for (path, source) in files {
        let target = dir.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("dirs");
        }
        std::fs::write(target, source).expect("write");
    }

    let config: ScriptConfig = serde_json::from_str(
        r#"{"id":"00000000-0000-0000-0000-000000000000",
            "entryPoint":"main.lua","includes":["**/*.lua"],"ignore":[]}"#,
    )
    .expect("config parses");
    let mut config = config;
    config.project_path = Some(dir.to_path_buf());
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_point_at_the_users_lines() {
        let offset = prologue().lines().count();
        let raw = format!("./main.lua({},7): TypeError: nope", offset + 2);
        assert_eq!(shift_line(&raw, offset), "./main.lua(2,7): TypeError: nope");

        // Lines without a position pass through untouched.
        assert_eq!(shift_line("some banner", offset), "some banner");
    }

    #[test]
    fn the_shipped_templates_type_check() {
        // Requires luau-analyze on PATH, which the dev shell provides. The
        // templates are what `actias init` hands every new user; a type
        // error in them would greet everyone.
        let dir = tempfile::tempdir().expect("tempdir");
        let config = project(
            dir.path(),
            &[(
                "main.lua",
                include_str!("../template/templates/basic/main.lua"),
            )],
        );
        analyze(&config).expect("the basic template checks");

        let dir = tempfile::tempdir().expect("tempdir");
        let config = project(
            dir.path(),
            &[
                (
                    "main.lua",
                    include_str!("../template/templates/router/main.lua"),
                ),
                (
                    "utils/router.lua",
                    include_str!("../template/templates/router/utils/router.lua"),
                ),
            ],
        );
        analyze(&config).expect("the router template checks");
    }

    #[test]
    fn a_type_error_in_a_template_variant_fails_check() {
        // The template's handler under `--!strict`, with one annotation the
        // body violates; the directive is what opts the file into types.
        let dir = tempfile::tempdir().expect("tempdir");
        let config = project(
            dir.path(),
            &[(
                "main.lua",
                r#"--!strict
local greeting: string = 5
on "fetch" (function(request)
    return { body = greeting }
end)
"#,
            )],
        );

        let error = analyze(&config).expect_err("the type error must fail check");
        assert!(error.contains("type check"), "{error}");
    }

    #[test]
    fn a_typo_against_the_platform_surface_fails_even_untyped() {
        // No directive at all: nonstrict still knows the ambient surface,
        // so a misspelled global is an error, not a runtime surprise.
        let dir = tempfile::tempdir().expect("tempdir");
        let config = project(
            dir.path(),
            &[(
                "main.lua",
                r#"on "fetch" (function() return { body = jsn.stringify({}) } end)"#,
            )],
        );

        analyze(&config).expect_err("an unknown global must fail check");
    }

    #[test]
    fn directives_hoist_above_the_prologue() {
        let shadow = shadow_source("--!strict\nlocal x = 1\nprint(x)\n");
        let first = shadow.lines().next().expect("has lines");
        assert_eq!(first, "--!strict");
        assert!(shadow.contains("local x = 1"));
    }
}
