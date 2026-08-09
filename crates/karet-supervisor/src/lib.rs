//! `karet-supervisor` — karet's hidden process modes.
//!
//! Two alternate `main()` entry points of the karet executable live here, out of
//! the headless `karet-session` library so embedders never compile them:
//!
//! - [`supervisor`] — crash-safe ownership for long-running external process
//!   trees. A hidden copy of the executable owns the real process group and
//!   kills it when the parent editor disappears, covering terminations no Rust
//!   destructor survives.
//! - [`broker`] — the cross-process language-server broker. One hidden broker
//!   owns each `(server launch, repository root)` pair; editor windows connect
//!   over an authenticated loopback socket so several karet instances share one
//!   expensive server process.
//!
//! The composition root checks [`broker::requested`] / [`supervisor::requested`]
//! first thing in `main()` and hands off to the matching `run_from_env`.

pub mod broker;
pub mod supervisor;
