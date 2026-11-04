//! `file-lines`: the 800-code-line ceiling for every Rust source file, counted
//! by `tokei`.

use std::io;
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

    parse_tokei_reports(&output.stdout)
}

/// The per-file Rust reports in `tokei --output json` output.
fn parse_tokei_reports(json: &[u8]) -> Result<Vec<RustReport>, String> {
    let result = serde_json::from_slice::<TokeiOutput>(json)
        .map_err(|error| format!("failed to parse tokei output: {error}"))?;
    Ok(result.rust.unwrap_or_default().reports)
}

/// Run the check: every Rust file in the workspace stays within the ceiling.
pub(crate) fn run() -> ExitCode {
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

    /// A fragment of real `tokei 12.1.2 --output json --type Rust` output (the
    /// version `mise.toml` pins), trimmed to two reports.
    const TOKEI_12_1_2_FRAGMENT: &str = r#"{
        "Rust": {
            "blanks": 30,
            "children": {
                "Markdown": [
                    {"name": "src/big.rs", "stats": {"blanks": 1, "blobs": {}, "code": 0, "comments": 6}}
                ]
            },
            "code": 1601,
            "comments": 12,
            "inaccurate": false,
            "reports": [
                {
                    "name": "src/big.rs",
                    "stats": {
                        "blanks": 19,
                        "blobs": {"Markdown": {"blanks": 1, "blobs": {}, "code": 0, "comments": 6}},
                        "code": 801,
                        "comments": 11
                    }
                },
                {
                    "name": "src/fits.rs",
                    "stats": {"blanks": 11, "blobs": {}, "code": 800, "comments": 1}
                }
            ]
        },
        "Total": {
            "blanks": 30,
            "children": {"Rust": []},
            "code": 1601,
            "comments": 12,
            "inaccurate": false,
            "reports": []
        }
    }"#;

    #[test]
    fn tokei_output_yields_the_file_over_the_line_limit() -> Result<(), String> {
        let reports = parse_tokei_reports(TOKEI_12_1_2_FRAGMENT.as_bytes())?;

        assert_eq!(
            rust_file_offenders(&reports, RUST_FILE_LINE_LIMIT),
            vec![Offender {
                name: "src/big.rs".into(),
                code: 801,
            }]
        );
        Ok(())
    }
}
