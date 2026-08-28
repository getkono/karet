//! A real language-server process, for tests that need one.
//!
//! Every session-level LSP test injects an in-memory `Connector`, so the path
//! karet actually takes in production — the session's connector, the hidden
//! process supervisor, the shared broker, and a child on the other end of a
//! real pipe — has never been executed by a test. That is why a family of
//! launch defects survived: they all live between the connector and the child.
//!
//! This binary is that child. It dispatches the same three ways
//! `karet`'s own `main` does, so it can stand in for the editor executable
//! wherever a supervisor or broker is spawned, and otherwise serves LSP with a
//! behaviour chosen by argv. Each behaviour is a failure shape observed in the
//! wild rather than an invented one:
//!
//! | `--behavior` | The real server it imitates |
//! |---|---|
//! | `normal` | a working server |
//! | `banner` | bare `taplo`, which prints usage to stdout |
//! | `exit-now` | a wrapper script whose interpreter is missing |
//! | `exit-stderr` | `node cli.js` with an unresolved module |
//! | `die-after-handshake` | a server that crashes once initialized |
//! | `no-content-length` | a peer that frames its output wrongly |
//! | `garbage-json` | correct framing, unparseable body |
//! | `slow` | a server that never answers `initialize` |
//!
//! `garbage-json` and `slow` both end in the client's 30-second request
//! timeout, which is correct — a server that is merely slow to answer
//! `initialize` must not be written off — but too slow to spend in the merge
//! gate. They are here for reproducing a report by hand.
//! | `report` | `normal`, plus a record of how it was launched |
//!
//! `report` is the argv oracle: it writes its own argv, working directory and
//! the launch-relevant environment to the file named by `--report`, which
//! proves the whole chain delivered what production intended, and doubles as a
//! launch counter for the broker's process-sharing tests.

use std::io::BufRead;
use std::io::Write;

/// One flag's value, when it is present in this process's argv.
fn flag(name: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        if argument == name {
            return args.next();
        }
    }
    None
}

fn main() {
    // The same order as `karet`'s main: a hidden mode owns the process before
    // anything else looks at argv.
    if karet_supervisor::broker::requested() {
        std::process::exit(karet_supervisor::broker::run_from_env());
    }
    if karet_supervisor::supervisor::requested() {
        std::process::exit(karet_supervisor::supervisor::run_from_env());
    }
    std::process::exit(serve());
}

fn behavior() -> String {
    flag("--behavior").unwrap_or_else(|| "normal".to_owned())
}

fn serve() -> i32 {
    let behavior = behavior();
    if behavior == "report" {
        record_launch();
    }
    match behavior.as_str() {
        "exit-now" => return 0,
        "exit-stderr" => {
            let _ = writeln!(
                std::io::stderr(),
                "node:internal/modules/cjs/loader:1215\n  throw err;\nError: Cannot find module \
                 'vscode-languageserver'"
            );
            return 1;
        },
        "banner" => {
            // Straight to stdout, ahead of any frame: this is what a CLI that
            // does not understand `--stdio` does, and it is fatal to framing.
            let _ = writeln!(
                std::io::stdout(),
                "taplo 0.10.0\nUsage: taplo [OPTIONS] <COMMAND>"
            );
            let _ = std::io::stdout().flush();
        },
        "slow" => {
            std::thread::sleep(std::time::Duration::from_secs(60));
            return 0;
        },
        _ => {},
    }
    conversation(&behavior)
}

/// Read framed requests and answer them until stdin ends.
fn conversation(behavior: &str) -> i32 {
    let stdin = std::io::stdin();
    let mut reader = std::io::BufReader::new(stdin.lock());
    let mut initialized = false;
    loop {
        let Some(message) = read_frame(&mut reader) else {
            return 0;
        };
        let Some(id) = message_id(&message) else {
            continue; // a notification; nothing to answer
        };
        let method = message_method(&message).unwrap_or_default();
        match method.as_str() {
            "initialize" => {
                match behavior {
                    "no-content-length" => {
                        let _ = write!(std::io::stdout(), "Content-Type: application/json\r\n\r\n");
                        let _ = std::io::stdout().flush();
                        return 0;
                    },
                    "garbage-json" => {
                        write_frame(b"this is not json");
                        continue;
                    },
                    _ => {},
                }
                write_frame(
                    format!(
                        r#"{{"jsonrpc":"2.0","id":{id},"result":{{"capabilities":{{"textDocumentSync":1,"completionProvider":{{}}}},"serverInfo":{{"name":"karet-testbed"}}}}}}"#
                    )
                    .as_bytes(),
                );
                initialized = true;
                if behavior == "die-after-handshake" {
                    return 0;
                }
            },
            "shutdown" => {
                write_frame(format!(r#"{{"jsonrpc":"2.0","id":{id},"result":null}}"#).as_bytes());
            },
            "textDocument/completion" if initialized => {
                write_frame(
                    format!(
                        r#"{{"jsonrpc":"2.0","id":{id},"result":[{{"label":"karet_testbed_item"}}]}}"#
                    )
                    .as_bytes(),
                );
            },
            _ => {
                write_frame(format!(r#"{{"jsonrpc":"2.0","id":{id},"result":null}}"#).as_bytes());
            },
        }
    }
}

/// Append this launch to the report file, for tests that assert on argv, cwd,
/// or how many times a process was actually started.
fn record_launch() {
    // Carried in argv rather than the environment: tests run in parallel
    // threads of one process, so a shared env var would let one test's report
    // path overwrite another's.
    let Some(path) = flag("--report") else {
        return;
    };
    let argv = std::env::args().collect::<Vec<_>>();
    let cwd = std::env::current_dir().unwrap_or_default();
    // The supervisor must scrub its own hidden-mode variables before spawning
    // the real server, or a descendant would re-enter supervisor mode.
    let inherited = [
        "KARET_INTERNAL_PROCESS_SUPERVISOR",
        "KARET_INTERNAL_PROCESS_SPEC",
    ]
    .into_iter()
    .filter(|name| std::env::var_os(name).is_some())
    .collect::<Vec<_>>();
    let line = serde_json::json!({
        "argv": argv,
        "cwd": cwd.to_string_lossy(),
        "leaked_env": inherited,
    });
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        // One record, one `write(2)`. `writeln!` goes through `write_fmt`,
        // which issues a syscall per formatting fragment, and `Display` for a
        // `serde_json::Value` emits an object as many small fragments -- so a
        // reader polling this file could observe a half-written line, and two
        // testbed processes appending to the same report (the broker's
        // process-sharing test) could interleave mid-record. `O_APPEND` makes a
        // single `write` atomic for a record this size, so build the whole line
        // in memory first.
        let _ = file.write_all(format!("{line}\n").as_bytes());
    }
}

fn read_frame(reader: &mut impl BufRead) -> Option<String> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0_u8; length?];
    reader.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

fn write_frame(body: &[u8]) {
    let mut stdout = std::io::stdout();
    let _ = write!(stdout, "Content-Length: {}\r\n\r\n", body.len());
    let _ = stdout.write_all(body);
    let _ = stdout.flush();
}

fn message_id(message: &str) -> Option<i64> {
    serde_json::from_str::<serde_json::Value>(message)
        .ok()?
        .get("id")?
        .as_i64()
}

fn message_method(message: &str) -> Option<String> {
    Some(
        serde_json::from_str::<serde_json::Value>(message)
            .ok()?
            .get("method")?
            .as_str()?
            .to_owned(),
    )
}
