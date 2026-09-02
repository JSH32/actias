//! Talking to the Luau language service.
//!
//! The service is `actias-luau`, built from `luau-web/`, and it is the
//! same implementation the workbench loads as wasm. Holding one process
//! open matters: the frontend keeps the checked project in memory, so a
//! second question about an unchanged file costs a lookup rather than a
//! re-parse of everything.
//!
//! Every position crossing this boundary is one-based and refers to the
//! shadowed text. [`Shadow`] owns the translation to and from the lines
//! a user actually wrote.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// One diagnostic over the shadowed text. The service spells the end of
/// a range in camel case, so the rename is load-bearing: without it the
/// range silently collapses to the start and every squiggle is a point.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub severity: String,
    pub message: String,
}

/// A source file rewritten for the checker, and the arithmetic to get
/// back. `shadow_source` hoists the file's own `--!` directives above the
/// prologue, because Luau only honours them at the very top, so the map
/// is in two pieces rather than one offset.
pub struct Shadow {
    /// Lines of `--!` directives, which keep their original numbers.
    directives: usize,
    /// Lines the prologue adds below them.
    prologue: usize,
}

impl Shadow {
    pub fn new(source: &str, prologue: &str) -> Self {
        Self {
            directives: source
                .lines()
                .take_while(|line| line.trim_start().starts_with("--!"))
                .count(),
            prologue: prologue.lines().count(),
        }
    }

    /// A user's zero-based line to the service's one-based line.
    pub fn to_service(&self, line: usize) -> usize {
        if line < self.directives {
            line + 1
        } else {
            line + 1 + self.prologue
        }
    }

    /// The service's one-based line back to the user's zero-based line,
    /// or [`None`] when it lands inside the prologue and so belongs to
    /// no line the user wrote.
    pub fn to_user(&self, line: usize) -> Option<usize> {
        if line <= self.directives {
            return Some(line - 1);
        }
        if line <= self.directives + self.prologue {
            return None;
        }
        Some(line - self.prologue - 1)
    }
}

/// Where the service lives, if it is installed. `ACTIAS_LUAU` names it
/// outright; otherwise it is looked for beside this binary, which is how
/// a release archive ships the pair, and then on PATH.
pub fn locate() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os("ACTIAS_LUAU") {
        let named = PathBuf::from(named);
        return named.exists().then_some(named);
    }

    let name = if cfg!(windows) {
        "actias-luau.exe"
    } else {
        "actias-luau"
    };

    if let Ok(own) = std::env::current_exe()
        && let Some(beside) = own.parent().map(|dir| dir.join(name))
        && beside.exists()
    {
        return Some(beside);
    }

    // No --version to probe with, so ask it to serve and quit at once.
    let probe = Command::new(name)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    match probe {
        Ok(mut child) => {
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(b"quit\n");
            }
            let _ = child.wait();
            Some(PathBuf::from(name))
        }
        Err(_) => None,
    }
}

/// One live service process.
pub struct Service {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Service {
    /// Starts the service.
    ///
    /// # Errors
    /// Returns text when the process cannot be spawned or its pipes are
    /// not there.
    pub fn start(command: &std::path::Path) -> Result<Self, String> {
        let mut child = Command::new(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("the language service could not start: {error}"))?;

        let stdin = child.stdin.take().ok_or("the service has no stdin")?;
        let stdout = child.stdout.take().ok_or("the service has no stdout")?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    /// Loads or replaces one module's text.
    ///
    /// # Errors
    /// Returns text when the service cannot be written to.
    pub fn set_file(&mut self, path: &str, text: &str) -> Result<(), String> {
        writeln!(self.stdin, "set {} {}", path.len(), text.len()).map_err(pipe)?;
        self.stdin.write_all(path.as_bytes()).map_err(pipe)?;
        self.stdin.write_all(text.as_bytes()).map_err(pipe)?;
        self.stdin.flush().map_err(pipe)
    }

    /// Every diagnostic for one module, positioned in the shadowed text.
    ///
    /// # Errors
    /// Returns text when the service cannot answer.
    pub fn check(&mut self, module: &str) -> Result<Vec<Diagnostic>, String> {
        let answer = self.query("check", module, None)?;
        serde_json::from_value(answer).map_err(|error| format!("{module}: {error}"))
    }

    /// A positioned question: `hover`, `complete`, `definition` or
    /// `signature`. [`serde_json::Value::Null`] means the service had
    /// nothing to say there, which is not an error.
    ///
    /// # Errors
    /// Returns text when the service cannot answer.
    pub fn at(
        &mut self,
        op: &str,
        module: &str,
        line: usize,
        column: usize,
    ) -> Result<serde_json::Value, String> {
        self.query(op, module, Some((line, column)))
    }

    fn query(
        &mut self,
        op: &str,
        module: &str,
        position: Option<(usize, usize)>,
    ) -> Result<serde_json::Value, String> {
        match position {
            Some((line, column)) => {
                writeln!(self.stdin, "{op} {} {line} {column}", module.len()).map_err(pipe)?
            }
            None => writeln!(self.stdin, "{op} {}", module.len()).map_err(pipe)?,
        }
        self.stdin.write_all(module.as_bytes()).map_err(pipe)?;
        self.stdin.flush().map_err(pipe)?;

        let mut line = String::new();
        if self.stdout.read_line(&mut line).map_err(pipe)? == 0 {
            return Err("the language service stopped answering".to_owned());
        }
        serde_json::from_str(line.trim_end()).map_err(|error| format!("{op} {module}: {error}"))
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.stdin.write_all(b"quit\n");
        let _ = self.stdin.flush();
        let _ = self.child.wait();
    }
}

fn pipe(error: std::io::Error) -> String {
    format!("the language service stopped: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two-piece map is where an off-by-one would hide, so it is
    /// checked without a service in the way.
    #[test]
    fn positions_survive_the_round_trip() {
        // Two directive lines, a four-line prologue.
        let shadow = Shadow {
            directives: 2,
            prologue: 4,
        };

        // A directive keeps its own line.
        assert_eq!(shadow.to_service(0), 1);
        assert_eq!(shadow.to_user(1), Some(0));

        // Code below the prologue is pushed down by exactly its length.
        assert_eq!(shadow.to_service(2), 7);
        assert_eq!(shadow.to_user(7), Some(2));

        // Every user line survives a round trip.
        for line in 0..40 {
            assert_eq!(shadow.to_user(shadow.to_service(line)), Some(line));
        }

        // The prologue's own lines belong to nobody.
        for line in 3..=6 {
            assert_eq!(shadow.to_user(line), None);
        }
    }
}
