//! Kernelspec discovery: the `kernels/<name>/kernel.json` layout Jupyter
//! installs into a small set of well-known directories.

use std::path::Path;
use std::path::PathBuf;

/// One installed kernel, read from its `kernel.json`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct KernelSpec {
    /// The launch argv; `{connection_file}` is substituted before spawn.
    pub argv: Vec<String>,
    /// The human-readable name (`Python 3 (ipykernel)`).
    #[serde(default)]
    pub display_name: String,
    /// The implementation language (`python`, `julia`, …).
    #[serde(default)]
    pub language: String,
    /// The spec directory name (`python3`), karet-assigned from the layout.
    #[serde(skip)]
    pub name: String,
    /// The spec directory, for resource resolution.
    #[serde(skip)]
    pub dir: PathBuf,
}

/// The standard kernelspec directories, most specific first: every
/// `$JUPYTER_PATH` entry, the user's data dir, then the system dirs.
#[must_use]
pub fn default_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(paths) = std::env::var_os("JUPYTER_PATH") {
        dirs.extend(std::env::split_paths(&paths).map(|path| path.join("kernels")));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        #[cfg(target_os = "macos")]
        dirs.push(home.join("Library/Jupyter/kernels"));
        #[cfg(not(target_os = "macos"))]
        dirs.push(home.join(".local/share/jupyter/kernels"));
    }
    dirs.push(PathBuf::from("/usr/local/share/jupyter/kernels"));
    dirs.push(PathBuf::from("/usr/share/jupyter/kernels"));
    dirs
}

/// Discover every kernelspec under the standard directories.
#[must_use]
pub fn discover() -> Vec<KernelSpec> {
    discover_in(&default_dirs())
}

/// Discover kernelspecs under explicit `dirs` (earlier directories win a
/// name; the testable core of [`discover`]).
#[must_use]
pub fn discover_in(dirs: &[PathBuf]) -> Vec<KernelSpec> {
    let mut specs: Vec<KernelSpec> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let spec_dir = entry.path();
            let Some(name) = spec_dir.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if specs.iter().any(|spec| spec.name == name) {
                continue; // an earlier (more specific) dir already claimed it
            }
            if let Some(spec) = read_spec(&spec_dir, name) {
                specs.push(spec);
            }
        }
    }
    specs.sort_by(|a, b| a.name.cmp(&b.name));
    specs
}

/// Pick a kernelspec for a notebook: an exact spec-name match first (the
/// notebook's `kernelspec.name`), then the first spec whose language matches.
#[must_use]
pub fn find<'a>(specs: &'a [KernelSpec], name: &str, language: &str) -> Option<&'a KernelSpec> {
    specs.iter().find(|spec| spec.name == name).or_else(|| {
        specs
            .iter()
            .find(|spec| spec.language.eq_ignore_ascii_case(language))
    })
}

/// Read one `kernel.json`; `None` for directories that are not kernelspecs.
fn read_spec(dir: &Path, name: &str) -> Option<KernelSpec> {
    let text = std::fs::read_to_string(dir.join("kernel.json")).ok()?;
    let mut spec: KernelSpec = serde_json::from_str(&text).ok()?;
    if spec.argv.is_empty() {
        return None;
    }
    spec.name = name.to_owned();
    spec.dir = dir.to_path_buf();
    Some(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_spec(root: &Path, name: &str, json: &str) {
        let dir = root.join(name);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("kernel.json"), json);
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("karet-kernelspec-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn discovers_specs_and_earlier_dirs_win() {
        let user = scratch("user");
        let system = scratch("system");
        write_spec(
            &user,
            "python3",
            r#"{"argv": ["python", "-m", "ipykernel_launcher", "-f", "{connection_file}"],
                "display_name": "User Python", "language": "python"}"#,
        );
        write_spec(
            &system,
            "python3",
            r#"{"argv": ["sys-python"], "display_name": "System Python", "language": "python"}"#,
        );
        write_spec(
            &system,
            "julia-1.10",
            r#"{"argv": ["julia", "-f", "{connection_file}"], "language": "julia",
                "display_name": "Julia"}"#,
        );
        write_spec(&system, "broken", r#"{"display_name": "no argv"}"#);
        let specs = discover_in(&[user.clone(), system.clone()]);
        assert_eq!(specs.len(), 2, "{specs:?}");
        let python = specs.iter().find(|spec| spec.name == "python3");
        assert_eq!(
            python.map(|spec| spec.display_name.as_str()),
            Some("User Python"),
            "the more specific dir wins"
        );
        let _ = std::fs::remove_dir_all(&user);
        let _ = std::fs::remove_dir_all(&system);
    }

    #[test]
    fn find_prefers_the_named_spec_then_the_language() {
        let spec = |name: &str, language: &str| KernelSpec {
            argv: vec!["x".to_owned()],
            display_name: String::new(),
            language: language.to_owned(),
            name: name.to_owned(),
            dir: PathBuf::new(),
        };
        let specs = vec![spec("python3", "python"), spec("xeus-cling", "c++")];
        assert_eq!(
            find(&specs, "xeus-cling", "python").map(|s| s.name.as_str()),
            Some("xeus-cling"),
            "the notebook's named spec wins over language"
        );
        assert_eq!(
            find(&specs, "missing", "C++").map(|s| s.name.as_str()),
            Some("xeus-cling"),
            "language matches case-insensitively"
        );
        assert_eq!(find(&specs, "missing", "r"), None);
    }
}
