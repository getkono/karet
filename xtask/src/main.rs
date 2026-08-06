//! Cross-platform development tasks for the karet workspace.

mod readme_svg;

use std::env;
use std::io;
use std::io::Read;
use std::process::Command;
use std::process::ExitCode;

use serde::Deserialize;

const RUST_FILE_LINE_LIMIT: usize = 800;

#[derive(Debug, Deserialize)]
struct RustReport {
    name: String,
    stats: CodeStats,
}

#[derive(Debug, Deserialize)]
struct CodeStats {
    code: usize,
}

#[derive(Debug, Default, Deserialize)]
struct Language {
    #[serde(default)]
    reports: Vec<RustReport>,
}

#[derive(Debug, Deserialize)]
struct TokeiOutput {
    #[serde(rename = "Rust")]
    rust: Option<Language>,
}

#[derive(Debug, Eq, PartialEq)]
struct Offender {
    name: String,
    code: usize,
}

/// Accessible name for the README hero, describing what the capture shows.
const HERO_TITLE: &str = "karet, a terminal code editor";
/// Accessible description for the README hero.
const HERO_DESCRIPTION: &str = "A karet window: the file explorer on the left, a Rust source \
                                file with syntax highlighting in the editor, and the status bar \
                                along the bottom.";

/// Convert a `karet --capture` grid on stdin into `assets/karet.svg`.
///
/// Reading the capture from a pipe (rather than a path) keeps the whole pipeline in
/// one command and leaves no intermediate file to go stale — see `scripts/gen-svg.sh`.
fn generate_readme_svg() -> ExitCode {
    let mut capture = String::new();
    if let Err(error) = io::stdin().read_to_string(&mut capture) {
        eprintln!("error: failed to read the capture from stdin: {error}");
        return ExitCode::from(2);
    }
    if capture.trim().is_empty() {
        eprintln!(
            "error: no capture on stdin\n       \
             usage: karet --capture … | cargo run --package xtask -- readme-svg"
        );
        return ExitCode::from(2);
    }
    let svg = match readme_svg::from_capture(&capture, HERO_TITLE, HERO_DESCRIPTION) {
        Ok(svg) => svg,
        Err(error) => {
            eprintln!("error: failed to render the capture: {error}");
            return ExitCode::from(2);
        },
    };

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("assets/karet.svg");
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("error: failed to create {}: {error}", parent.display());
        return ExitCode::from(2);
    }
    if let Err(error) = std::fs::write(&path, &svg) {
        eprintln!("error: failed to write {}: {error}", path.display());
        return ExitCode::from(2);
    }
    println!("Wrote {} ({} bytes)", path.display(), svg.len());
    ExitCode::SUCCESS
}

fn rust_file_offenders(reports: &[RustReport], limit: usize) -> Vec<Offender> {
    let mut offenders = reports
        .iter()
        .filter(|report| report.stats.code > limit)
        .map(|report| Offender {
            name: report.name.clone(),
            code: report.stats.code,
        })
        .collect::<Vec<_>>();

    offenders.sort_by(|left, right| {
        right
            .code
            .cmp(&left.code)
            .then_with(|| left.name.cmp(&right.name))
    });
    offenders
}

fn tokei_rust_reports() -> Result<Vec<RustReport>, String> {
    let output = Command::new("tokei")
        .args(["--output", "json", "--type", "Rust", "."])
        .output()
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => {
                "tokei is required; install workspace tools with `mise install`".to_owned()
            },
            _ => format!("failed to run tokei: {error}"),
        })?;
    if !output.status.success() {
        return Err(format!(
            "tokei failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let result = serde_json::from_slice::<TokeiOutput>(&output.stdout)
        .map_err(|error| format!("failed to parse tokei output: {error}"))?;
    Ok(result.rust.unwrap_or_default().reports)
}

fn check_rust_file_lines() -> ExitCode {
    let reports = match tokei_rust_reports() {
        Ok(reports) => reports,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        },
    };
    let offenders = rust_file_offenders(&reports, RUST_FILE_LINE_LIMIT);
    if offenders.is_empty() {
        println!("All Rust files are within the {RUST_FILE_LINE_LIMIT}-code-line limit.");
        return ExitCode::SUCCESS;
    }

    eprintln!("Rust files exceeding the {RUST_FILE_LINE_LIMIT}-code-line limit:");
    for offender in offenders {
        eprintln!("{}: {} code lines", offender.name, offender.code);
    }
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some(command), None) if command == "file-lines" => check_rust_file_lines(),
        (Some(command), None) if command == "readme-svg" => generate_readme_svg(),
        _ => {
            eprintln!("usage: cargo run --package xtask -- <file-lines|readme-svg>");
            ExitCode::from(2)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(name: &str, code: usize) -> RustReport {
        RustReport {
            name: name.to_owned(),
            stats: CodeStats { code },
        }
    }

    #[test]
    fn offenders_are_over_limit_and_sorted_by_size_then_name() {
        let reports = vec![
            report("within.rs", 800),
            report("b.rs", 801),
            report("largest.rs", 900),
            report("a.rs", 801),
        ];

        assert_eq!(
            rust_file_offenders(&reports, 800),
            vec![
                Offender {
                    name: "largest.rs".into(),
                    code: 900,
                },
                Offender {
                    name: "a.rs".into(),
                    code: 801,
                },
                Offender {
                    name: "b.rs".into(),
                    code: 801,
                },
            ]
        );
    }
}
