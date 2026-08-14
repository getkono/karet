//! Time a workspace spelling scan end to end, through the public backend seam.
//!
//! Issue #182 asked whether spell-checking a large repository is feasible at all
//! before building a view on top of it. This measures exactly what a user waits
//! for — command in, `SpellingScanFinished` out — rather than a synthetic inner
//! loop, so the number stays honest as the pipeline changes.
//!
//! ```sh
//! cargo run --release -p karet-session --example spell-scan-bench -- [ROOT] [LIMIT]
//! ```
//!
//! `ROOT` defaults to the current directory and `LIMIT` to 100000 (high enough that
//! a normal run measures the whole tree rather than stopping early). Needs a
//! `en_US` Hunspell dictionary on the usual search path — the same requirement the
//! feature itself has; without one the scan reports the missing dictionary and
//! finishes with nothing scanned.

use std::path::PathBuf;
use std::time::Instant;

use karet_session::Backend;
use karet_session::Command;
use karet_session::Event;
use karet_session::SessionConfig;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let root = args
        .next()
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .canonicalize()
        .unwrap_or_else(|error| {
            eprintln!("spell-scan-bench: cannot resolve root: {error}");
            std::process::exit(2);
        });
    let limit: usize = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(100_000);

    let mut settings = karet_session::Settings::default();
    settings.spellcheck.enabled = true;
    settings.spellcheck.language = "en_US".to_owned();

    let (backend, _snapshots) = karet_session::local(SessionConfig {
        roots: vec![root.clone()],
        settings,
        ..SessionConfig::default()
    });
    let Some(mut events) = backend.take_events() else {
        eprintln!("spell-scan-bench: the backend has no event stream");
        std::process::exit(2);
    };

    println!("root:  {}", root.display());
    println!("limit: {limit} hits");

    let id = backend.next_id();
    let started = Instant::now();
    if backend
        .send(id, Command::ScanWorkspaceSpelling { limit })
        .is_err()
    {
        eprintln!("spell-scan-bench: the backend refused the command");
        std::process::exit(2);
    }

    let mut first_batch = None;
    let mut batches = 0_usize;
    let mut hits = 0_usize;
    while let Some((answers, event)) = events.recv().await {
        if answers != Some(id) {
            continue; // startup chatter: VCS status, config diagnostics, …
        }
        match event {
            Event::SpellingScanProgress { hits: batch, .. } => {
                first_batch.get_or_insert_with(|| started.elapsed());
                batches += 1;
                hits += batch.len();
            },
            Event::SpellingScanFinished {
                files_scanned,
                truncated,
                cancelled,
            } => {
                let elapsed = started.elapsed();
                println!("files scanned:  {files_scanned}");
                println!("misspellings:   {hits} in {batches} streamed batches");
                match first_batch {
                    Some(first) => println!("first batch:    {first:.2?}"),
                    None => println!("first batch:    (none — nothing was flagged)"),
                }
                println!("total:          {elapsed:.2?}");
                if truncated {
                    println!("NOTE: stopped at the {limit}-hit limit; raise LIMIT to scan it all");
                }
                if cancelled {
                    println!("NOTE: the scan was cancelled");
                }
                return;
            },
            Event::Notification { message, .. } => println!("note: {message}"),
            _ => {},
        }
    }
    eprintln!("spell-scan-bench: the session closed before the scan finished");
    std::process::exit(1);
}
