//! Print grouped semantic blame for a file as JSON — the shape meant to be piped
//! into another tool or an AI agent.
//!
//! ```sh
//! cargo run -p blameline --example blame -- path/to/file.rs
//! ```

use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arg = std::env::args().nth(1).ok_or("usage: blame <file>")?;
    // Resolve to an absolute path so blame works regardless of the current directory.
    let file = std::fs::canonicalize(PathBuf::from(arg))?;
    let root = file.parent().unwrap_or(Path::new("."));
    let groups = blameline::blame_file(root, &file)?;
    println!("{}", blameline::to_json(&groups)?);
    Ok(())
}
