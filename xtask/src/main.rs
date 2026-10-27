//! Cross-platform development tasks for the karet workspace.
//!
//! This file only dispatches: each task lives in its own module.

mod file_lines;
mod publish;
mod readme_svg;

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some(command), None) if command == "file-lines" => file_lines::run(),
        (Some(command), None) if command == "publish-closure" => publish::check_closure(),
        (Some(command), None) if command == "publish-ready" => publish::check_ready(),
        (Some(command), None) if command == "readme-svg" => readme_svg::run(),
        _ => {
            eprintln!(
                "usage: cargo run --package xtask -- \
                 <file-lines|publish-closure|publish-ready|readme-svg>"
            );
            ExitCode::from(2)
        },
    }
}
