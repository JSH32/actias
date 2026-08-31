//! How the cli speaks.
//!
//! Cargo's shape: a right-aligned past-tense verb in colour, then the
//! thing it happened to. The alignment is what makes a run scannable, so
//! nothing else needs a marker and only failure gets one.

use colored::Colorize;
use std::fmt::Display;

/// Cargo's column. Verbs longer than this push their line right rather
/// than being truncated, which is rare and harmless.
const VERB_WIDTH: usize = 12;

/// Colour has to go on after padding: a coloured string carries ansi
/// escapes, and format's width counts those bytes.
fn line(verb: &str, painted: impl Fn(String) -> colored::ColoredString, rest: impl Display) {
    println!("{} {}", painted(format!("{verb:>VERB_WIDTH$}")), rest);
}

/// Something happened and it worked.
pub fn done(verb: &str, rest: impl Display) {
    line(verb, |text| text.green().bold(), rest);
}

/// Something is happening, or is context for the line above it.
pub fn step(verb: &str, rest: impl Display) {
    line(verb, |text| text.cyan().bold(), rest);
}

/// A detail hanging off the line above, with the verb column left empty
/// so the eye keeps following one edge.
pub fn detail(rest: impl Display) {
    println!("{:>VERB_WIDTH$} {}", "", rest);
}

/// Something the run survived but the reader should know about.
pub fn warn(rest: impl Display) {
    println!("{}: {}", "warning".yellow().bold(), rest);
}

/// Something that stopped the run.
pub fn error(kind: &str, rest: impl Display) {
    println!("{}: {}", kind.red().bold(), rest);
}
