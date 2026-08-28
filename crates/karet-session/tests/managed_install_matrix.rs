//! Does every managed language server actually install and start, today?
//!
//! Everything else in this workspace tests karet against fixtures. This tests
//! karet against upstream, which is the only way to notice that a release asset
//! was renamed, a publisher stopped attaching a digest, or an npm flag was
//! removed. Those failures reach users as "the server just doesn't work" and are
//! invisible to any offline suite -- the macOS and Windows Node key mismatch
//! this branch fixes sat in the catalogue undetected for exactly that reason.
//!
//! It is `#[ignore]`d, which is the only mechanism `cargo test` honours by
//! default, so `mise run verify` provably cannot reach the network. Run it
//! deliberately:
//!
//! ```text
//! mise run test-servers-live
//! KARET_LIVE_SERVERS=bash-language-server,taplo mise run test-servers-live
//! ```
//!
//! It installs each provider into a throwaway registry, opens a file of that
//! provider's language, waits for the connection to reach `Running`, and prints
//! a table. Failures are reported together at the end rather than aborting on
//! the first, so one broken upstream does not hide the rest.

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use karet_session::api::Command;
use karet_session::api::Event;
use karet_session::api::LanguageServerId;
use karet_session::api::LanguageServerRuntimeState;
use karet_session::backend::Backend;
use karet_session::backend::local;
use karet_session::session::SessionConfig;

/// Downloading a Node runtime plus a package, or a large native release, over
/// an ordinary connection.
const INSTALL_DEADLINE: Duration = Duration::from_secs(600);
/// A server that has been installed should connect quickly; some index first.
const LAUNCH_DEADLINE: Duration = Duration::from_secs(120);

/// A file each provider's language is recognized by, so the launch half of the
/// matrix can open something that routes to it.
///
/// Keyed by provider rather than language because that is what the row is
/// about. A provider absent here is installed and then **fails** the suite as
/// `not launched` rather than being silently skipped: a new recipe that nothing
/// here can open has not been shown to work.
const SAMPLE_FILES: &[(&str, &str, &str)] = &[
    ("rust-analyzer", "main.rs", "fn main() {}\n"),
    (
        "typescript-language-server",
        "index.ts",
        "export const a = 1;\n",
    ),
    ("pyright", "main.py", "x = 1\n"),
    ("ruff", "main.py", "x = 1\n"),
    ("texlab", "main.tex", "\\documentclass{article}\n"),
    ("clangd", "main.c", "int main(void) { return 0; }\n"),
    ("zls", "main.zig", "pub fn main() void {}\n"),
    ("astro-language-server", "page.astro", "<h1>hi</h1>\n"),
    ("svelte-language-server", "App.svelte", "<h1>hi</h1>\n"),
    (
        "vue-language-server",
        "App.vue",
        "<template><p/></template>\n",
    ),
    ("yaml-language-server", "config.yaml", "a: 1\n"),
    (
        "vscode-html-language-server",
        "index.html",
        "<html></html>\n",
    ),
    (
        "vscode-css-language-server",
        "style.css",
        "a { color: red }\n",
    ),
    ("vscode-json-language-server", "data.json", "{}\n"),
    ("bash-language-server", "script.sh", "echo hi\n"),
    ("docker-langserver", "Dockerfile", "FROM scratch\n"),
    ("graphql-lsp", "schema.graphql", "type Query { a: Int }\n"),
    ("biome", "index.js", "export const a = 1;\n"),
    ("lua-language-server", "main.lua", "print('hi')\n"),
    ("clojure-lsp", "main.clj", "(println 1)\n"),
    ("buf", "api.proto", "syntax = \"proto3\";\n"),
    ("marksman", "README.md", "# hi\n"),
    ("neocmakelsp", "CMakeLists.txt", "project(x)\n"),
];

#[derive(Default)]
struct Row {
    server: String,
    version: String,
    installed: Outcome,
    launched: Outcome,
    seconds: u64,
    note: String,
}

#[derive(Default, PartialEq, Eq)]
enum Outcome {
    Ok,
    Failed,
    #[default]
    Skipped,
}

impl Outcome {
    fn mark(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "FAILED",
            Self::Skipped => "-",
        }
    }
}

fn sample_for(server: &str) -> Option<(&'static str, &'static str)> {
    SAMPLE_FILES
        .iter()
        .find(|(key, ..)| *key == server)
        .map(|(_, name, body)| (*name, *body))
}

/// The prefix every registry failure reaches the client under.
///
/// `RegistryUpdate::Failed` is the single path a failed install takes, and it is
/// emitted as one error notification formatted `language-server registry: {…}`.
const REGISTRY_FAILURE_PREFIX: &str = "language-server registry:";

/// Does this notification report the install failure `key`'s row is waiting on?
///
/// Matching `LanguageServerId::display_name` was wrong twice over. The registry
/// names the *key* -- `ruff`, not "Ruff"; `typescript-language-server`, not
/// "TypeScript Language Server" -- so eight managed providers could never
/// match. Worse, every Node-side error ("Node publishes no active LTS release",
/// "Node {v} does not publish {key}", "Node checksum manifest has no {file}")
/// names no server at all, which is all twelve npm-backed providers and exactly
/// the upstream drift this file exists to catch. Each row drives its own
/// throwaway backend with exactly one install in flight, so a registry failure
/// that names nothing still belongs to the row that is waiting.
fn reports_install_failure(message: &str, key: &str) -> bool {
    message.contains(REGISTRY_FAILURE_PREFIX) || message.contains(key)
}

/// The providers whose row did not come out clean.
///
/// `Skipped` counts as a failure on both halves. A recipe with no `SAMPLE_FILES`
/// entry leaves `launched` at its default, and counting only `Failed` meant a
/// twenty-fourth recipe added without a sample would print `-` and pass green
/// for ever -- the opposite of what this module documents.
fn failing_rows(rows: &[Row]) -> Vec<&str> {
    rows.iter()
        .filter(|row| row.installed != Outcome::Ok || row.launched != Outcome::Ok)
        .map(|row| row.server.as_str())
        .collect()
}

/// Repository markers a companion provider needs before karet attaches it.
///
/// `ruff` and `biome` are not any language's default provider: they are
/// selected per document by a marker in the repository, so a workspace without
/// one legitimately never starts them. Writing the marker is what makes the
/// launch half of their row mean anything.
const REPOSITORY_MARKERS: &[(&str, &str, &str)] = &[
    ("ruff", "ruff.toml", "line-length = 88\n"),
    ("biome", "biome.json", "{}\n"),
];

fn markers_for(server: &str) -> Option<(&'static str, &'static str)> {
    REPOSITORY_MARKERS
        .iter()
        .find(|(key, ..)| *key == server)
        .map(|(_, name, body)| (*name, *body))
}

/// The providers karet claims it can install on this platform, asked through
/// the same public seam the Language Servers panel uses.
async fn managed_servers() -> Vec<LanguageServerId> {
    let Some(workspace) = tempfile::tempdir().ok() else {
        return Vec::new();
    };
    let (backend, _snapshots) = local(SessionConfig {
        lsp_registry_dir: Some(workspace.path().to_path_buf()),
        ..SessionConfig::default()
    });
    let Some(mut events) = backend.take_events() else {
        return Vec::new();
    };
    if backend
        .send(backend.next_id(), Command::LanguageServerStatus)
        .is_err()
    {
        return Vec::new();
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let Ok(Some((_, event))) =
            tokio::time::timeout(Duration::from_secs(30), events.recv()).await
        else {
            break;
        };
        if let Event::LanguageServerStatus { servers } = event {
            return servers
                .into_iter()
                .filter(|status| status.managed)
                .map(|status| status.server)
                .collect();
        }
    }
    Vec::new()
}

/// Install `server` into a throwaway registry, then open a file of its language
/// and wait for the connection to come up.
async fn exercise(server: &LanguageServerId, supervisor: &Path) -> Row {
    let started = Instant::now();
    let mut row = Row {
        server: server.key().to_owned(),
        ..Row::default()
    };
    let (Ok(registry), Ok(workspace)) = (tempfile::tempdir(), tempfile::tempdir()) else {
        row.note = "no temporary directory".to_owned();
        return row;
    };
    let (backend, _snapshots) = local(SessionConfig {
        process_supervisor: Some(supervisor.to_path_buf()),
        lsp_registry_dir: Some(registry.path().to_path_buf()),
        roots: vec![workspace.path().to_path_buf()],
        ..SessionConfig::default()
    });
    let Some(mut events) = backend.take_events() else {
        row.note = "no event stream".to_owned();
        return row;
    };

    if backend
        .send(
            backend.next_id(),
            Command::InstallLanguageServer {
                server: server.clone(),
            },
        )
        .is_err()
    {
        row.note = "install command refused".to_owned();
        return row;
    }

    row.installed = Outcome::Failed;
    let deadline = Instant::now() + INSTALL_DEADLINE;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Ok(Some((_, event))) = tokio::time::timeout(remaining, events.recv()).await else {
            row.note = "timed out installing".to_owned();
            break;
        };
        match event {
            Event::LanguageServerChanged {
                server: changed,
                version,
                ..
            } if changed == *server => {
                row.installed = Outcome::Ok;
                row.version = version;
                break;
            },
            Event::Notification { message, .. }
                if reports_install_failure(&message, server.key()) =>
            {
                row.note = message;
                break;
            },
            _ => {},
        }
    }
    row.seconds = started.elapsed().as_secs();
    if row.installed != Outcome::Ok {
        return row;
    }

    let Some((name, body)) = sample_for(server.key()) else {
        // A managed recipe with nothing to open cannot be shown to launch, and
        // an unlaunchable row is a failure, not a blank.
        row.launched = Outcome::Failed;
        row.note = "no sample file for this provider; add one to SAMPLE_FILES".to_owned();
        return row;
    };
    if let Some((marker, contents)) = markers_for(server.key()) {
        let _ = std::fs::write(workspace.path().join(marker), contents);
    }
    let path = workspace.path().join(name);
    if std::fs::write(&path, body).is_err() {
        row.note = "could not write the sample file".to_owned();
        return row;
    }
    if backend
        .send(
            backend.next_id(),
            Command::OpenDocument {
                path,
                language: None,
            },
        )
        .is_err()
    {
        row.note = "open refused".to_owned();
        return row;
    }

    row.launched = Outcome::Failed;
    let deadline = Instant::now() + LAUNCH_DEADLINE;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Ok(Some((_, event))) = tokio::time::timeout(remaining, events.recv()).await else {
            row.note = "timed out starting".to_owned();
            break;
        };
        match event {
            Event::LanguageServerRuntimeChanged { state, error, .. } => match state {
                LanguageServerRuntimeState::Running => {
                    row.launched = Outcome::Ok;
                    break;
                },
                LanguageServerRuntimeState::Unavailable
                | LanguageServerRuntimeState::CircuitOpen => {
                    row.note = error.unwrap_or_else(|| "did not start".to_owned());
                    break;
                },
                _ => {},
            },
            Event::Notification { message, .. } if message.contains("failed to start") => {
                row.note = message;
                break;
            },
            _ => {},
        }
    }
    row.seconds = started.elapsed().as_secs();
    row
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "installs every managed language server from upstream; run with `mise run test-servers-live`"]
async fn every_managed_language_server_installs_and_starts() {
    // `karet-testbed`, not this test binary: the npm install path runs `npm`
    // through the process supervisor, and only a binary that dispatches on the
    // hidden-mode environment can be one. Supervisor mode is
    // protocol-agnostic, so it wraps `node`, `npm` and each server exactly as
    // the editor would.
    let supervisor = PathBuf::from(env!("CARGO_BIN_EXE_karet-testbed"));

    let selected = std::env::var("KARET_LIVE_SERVERS").unwrap_or_default();
    let wanted = selected
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();

    let servers = managed_servers().await;
    assert!(
        !servers.is_empty(),
        "no managed providers on this platform; the catalogue or the status seam is broken"
    );

    let mut rows = Vec::new();
    for server in servers {
        if !wanted.is_empty() && !wanted.iter().any(|name| name == server.key()) {
            continue;
        }
        println!("--- {} ---", server.key());
        rows.push(exercise(&server, &supervisor).await);
    }

    let mut table = String::from(
        "\n| provider | version | install | launch | secs | note |\n\
         |---|---|---|---|---:|---|\n",
    );
    for row in &rows {
        table.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            row.server,
            if row.version.is_empty() {
                "-"
            } else {
                &row.version
            },
            row.installed.mark(),
            row.launched.mark(),
            row.seconds,
            row.note.replace('\n', " ").replace('|', "/"),
        ));
    }
    println!("{table}");

    let failed = failing_rows(&rows);
    assert!(
        failed.is_empty(),
        "these providers do not install or start against upstream today: {}\n{table}",
        failed.join(", ")
    );
}

// --- the matrix's own decisions, offline ----------------------------------
//
// Both are covered without the network, because a live run that mis-attributes
// a failure surfaces only as a ten-minute timeout with a useless note.

/// The registry names the key, and `display_name` remaps eight of them.
#[test]
fn a_failure_naming_the_key_is_attributed_to_its_row() {
    for key in [
        "ruff",
        "pyright",
        "typescript-language-server",
        "astro-language-server",
        "svelte-language-server",
        "vue-language-server",
        "yaml-language-server",
        "rust-analyzer",
    ] {
        let message = format!("{REGISTRY_FAILURE_PREFIX} release v1 has no {key}-linux.tar.gz");
        assert!(reports_install_failure(&message, key), "{message}");
    }
}

/// The Node-side errors -- which reach all twelve npm-backed providers -- name
/// no server at all.
#[test]
fn a_failure_naming_no_server_still_fails_the_waiting_row() {
    for message in [
        "language-server registry: Node publishes no active LTS release",
        "language-server registry: Node v22.11.0 does not publish osx-arm64-tar",
        "language-server registry: Node checksum manifest has no node-v22.11.0-darwin-arm64.tar.gz",
        "language-server registry: process supervisor is unavailable",
    ] {
        assert!(
            reports_install_failure(message, "bash-language-server"),
            "{message}"
        );
    }
}

/// Unrelated chatter must not end the wait early and blame the row.
#[test]
fn an_unrelated_notification_is_not_mistaken_for_a_failure() {
    assert!(!reports_install_failure(
        "workspace indexing finished",
        "ruff"
    ));
}

fn outcome_row(installed: Outcome, launched: Outcome) -> Row {
    Row {
        server: "example".to_owned(),
        installed,
        launched,
        ..Row::default()
    }
}

/// A recipe with no `SAMPLE_FILES` entry printed `-` and passed green for ever,
/// because the filter counted only `Failed`.
#[test]
fn a_row_that_never_launched_fails_the_suite() {
    let none: Vec<&str> = Vec::new();
    assert_eq!(failing_rows(&[outcome_row(Outcome::Ok, Outcome::Ok)]), none);
    for (installed, launched) in [
        (Outcome::Ok, Outcome::Skipped),
        (Outcome::Skipped, Outcome::Skipped),
        (Outcome::Ok, Outcome::Failed),
        (Outcome::Failed, Outcome::Ok),
    ] {
        assert_eq!(
            failing_rows(&[outcome_row(installed, launched)]),
            vec!["example"]
        );
    }
}
