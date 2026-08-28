//! jdtls launch polish: a stable per-project workspace directory and a JDK
//! preflight.
//!
//! Eclipse JDT.LS keeps its project index in the directory named by `-data`;
//! without one every launch re-imports the build from scratch (or trips over
//! another instance's default). The manager injects a cache-keyed default
//! when the user's args don't already name one. jdtls itself needs a modern
//! JDK to run, so a missing or old `java` is diagnosed up front instead of
//! surfacing as an opaque spawn failure.

use std::hash::Hash;
use std::hash::Hasher;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use super::LspSpec;

/// The minimum JDK major version jdtls itself runs on.
pub(crate) const MIN_JDK_MAJOR: u32 = 21;

impl super::LspManager {
    /// Gate a jdtls launch: probe the JDK once per generation (diagnosing a
    /// failure exactly once) and give the spec a stable workspace directory.
    /// `false` means the server must not be spawned.
    pub(super) fn jdtls_launch_gate(&mut self, spec: &mut LspSpec, root: &Path) -> bool {
        if !is_jdtls(spec) {
            return true;
        }
        let first_check = self.jdtls_preflight.is_none();
        let diagnosis = self.jdtls_preflight.get_or_insert_with(preflight_failure);
        if let Some(message) = diagnosis.clone() {
            // Diagnosed once, on the probe itself; the cached failure keeps
            // later calls quiet until reconfigure re-probes.
            if first_check {
                let _ = self.updates.send(super::LspUpdate::PreflightFailed {
                    generation: self.generation,
                    message,
                });
            }
            return false;
        }
        inject_data_arg(spec, root);
        true
    }
}

/// Whether the resolved spec launches Eclipse JDT.LS.
fn is_jdtls(spec: &LspSpec) -> bool {
    Path::new(&spec.command)
        .file_stem()
        .is_some_and(|stem| stem == "jdtls")
}

/// Append `-data <cache>/jdtls/<hash-of-root>` unless the args already name a
/// workspace directory.
fn inject_data_arg(spec: &mut LspSpec, root: &Path) {
    if spec
        .args
        .iter()
        .any(|arg| arg == "-data" || arg == "--data")
    {
        return;
    }
    let Some(dir) = workspace_data_dir(root) else {
        return;
    };
    spec.args.push("-data".to_owned());
    spec.args.push(dir.to_string_lossy().into_owned());
}

/// The stable per-project jdtls workspace directory (a cache: losing it only
/// costs a re-import, so the latex-style path hash is stable enough).
fn workspace_data_dir(root: &Path) -> Option<PathBuf> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut hasher);
    directories::ProjectDirs::from("", "", "karet").map(|dirs| {
        dirs.cache_dir()
            .join("jdtls")
            .join(format!("{:016x}", hasher.finish()))
    })
}

/// The user-facing diagnosis when the JDK preflight fails; `None` when jdtls
/// can run. Shells out to `java -version` — call once and cache.
fn preflight_failure() -> Option<String> {
    match detected_jdk_major() {
        Some(major) if major >= MIN_JDK_MAJOR => None,
        Some(major) => Some(format!(
            "jdtls needs a JDK {MIN_JDK_MAJOR} or newer to run, but `java` on PATH is version \
             {major} — install a newer JDK or put one first on PATH"
        )),
        None => Some(format!(
            "jdtls needs a JDK {MIN_JDK_MAJOR} or newer on PATH to run, but no working `java` \
             was found"
        )),
    }
}

/// The installed JDK's major version; `None` when `java` is missing or its
/// version banner is unparseable.
fn detected_jdk_major() -> Option<u32> {
    let output = Command::new("java").arg("-version").output().ok()?;
    // `java -version` historically prints its banner to stderr.
    let banner = if output.stderr.is_empty() {
        output.stdout
    } else {
        output.stderr
    };
    jdk_major(&String::from_utf8_lossy(&banner))
}

/// Parse the major version out of a `java -version` banner
/// (`openjdk version "21.0.2"`, `java version "1.8.0_392"`).
///
/// Anchored on the `version "…"` line rather than the first quote anywhere in
/// the output: the JVM prints `Picked up JAVA_TOOL_OPTIONS: …` ahead of the
/// banner, and a quoted value in those options would otherwise be read as the
/// version — reporting no working `java` on a perfectly good JDK.
fn jdk_major(banner: &str) -> Option<u32> {
    let quoted = banner
        .lines()
        .find_map(|line| line.split_once("version \""))
        .and_then(|(_, rest)| rest.split('"').next())?;
    let mut parts = quoted.split(['.', '-', '+', '_']);
    let first: u32 = parts.next()?.trim().parse().ok()?;
    // Pre-9 JDKs report as `1.<major>`.
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(command: &str, args: &[&str]) -> LspSpec {
        LspSpec {
            command: command.to_owned(),
            args: args.iter().map(|&arg| arg.to_owned()).collect(),
            languages: vec!["java".to_owned()],
            initialization_options: None,
        }
    }

    #[test]
    fn recognizes_jdtls_by_file_stem() {
        assert!(is_jdtls(&spec("jdtls", &[])));
        assert!(is_jdtls(&spec("/opt/jdtls/bin/jdtls", &[])));
        assert!(!is_jdtls(&spec("rust-analyzer", &[])));
        assert!(!is_jdtls(&spec("", &[])));
    }

    #[test]
    fn injects_data_when_absent() {
        let mut launch = spec("jdtls", &["--jvm-arg=-Xmx1G"]);
        inject_data_arg(&mut launch, Path::new("/work/project"));
        let data = launch.args.iter().position(|arg| arg == "-data");
        let index = data.unwrap_or(usize::MAX);
        assert_eq!(index, 1, "expected -data appended: {:?}", launch.args);
        let dir = launch.args.get(index + 1).cloned().unwrap_or_default();
        assert!(dir.contains("jdtls"), "workspace dir under jdtls/: {dir}");
    }

    #[test]
    fn respects_an_explicit_data_arg() {
        let mut launch = spec("jdtls", &["-data", "/tmp/ws"]);
        inject_data_arg(&mut launch, Path::new("/work/project"));
        assert_eq!(launch.args, vec!["-data", "/tmp/ws"]);
    }

    #[test]
    fn data_dir_is_stable_and_per_root() {
        let mut first = spec("jdtls", &[]);
        let mut again = spec("jdtls", &[]);
        let mut other = spec("jdtls", &[]);
        inject_data_arg(&mut first, Path::new("/work/a"));
        inject_data_arg(&mut again, Path::new("/work/a"));
        inject_data_arg(&mut other, Path::new("/work/b"));
        assert_eq!(first.args, again.args);
        assert_ne!(first.args, other.args);
    }

    #[test]
    fn parses_modern_and_legacy_version_banners() {
        assert_eq!(jdk_major("openjdk version \"21.0.2\" 2024-01-16"), Some(21));
        assert_eq!(jdk_major("openjdk version \"17\" 2021-09-14"), Some(17));
        assert_eq!(
            jdk_major("java version \"1.8.0_392\"\nJava(TM) SE Runtime"),
            Some(8)
        );
        assert_eq!(jdk_major("openjdk version \"22-ea\" 2024-03-19"), Some(22));
        assert_eq!(jdk_major("bash: java: command not found"), None);
        assert_eq!(jdk_major(""), None);
    }

    #[test]
    fn options_echoed_before_the_banner_do_not_confuse_the_parser() {
        // The JVM prints these to stderr ahead of the version banner whenever
        // JAVA_TOOL_OPTIONS/_JAVA_OPTIONS is set — common in CI and container
        // images. A quote in the echoed value used to be read as the version,
        // which reported no working `java` and blocked jdtls entirely.
        assert_eq!(
            jdk_major(
                "Picked up JAVA_TOOL_OPTIONS: -Dfile.encoding=UTF-8\nopenjdk version \"21.0.3\" 2024-04-16"
            ),
            Some(21)
        );
        assert_eq!(
            jdk_major(
                "Picked up JAVA_TOOL_OPTIONS: -Dhttp.agent=\"karet\"\nopenjdk version \"21.0.3\" 2024-04-16"
            ),
            Some(21)
        );
        assert_eq!(
            jdk_major("Picked up _JAVA_OPTIONS: -Xmx=\"2g\"\njava version \"1.8.0_392\""),
            Some(8)
        );
    }
}
