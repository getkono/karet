//! `karet-supervisor` — karet's hidden process modes.
//!
//! Two alternate `main()` entry points of the karet executable live here. The
//! app's composition root dispatches to them, and the headless `karet-session`
//! backend depends on this crate for process ownership: it spawns language
//! servers, debug adapters, notebook kernels and LaTeX builds through
//! [`supervisor::command`] / [`supervisor::blocking_command`] and connects to
//! the shared [`broker`].
//!
//! - [`supervisor`] — crash-safe ownership for long-running external process
//!   trees. A hidden copy of the executable owns the real process group and
//!   kills it when the parent editor disappears, covering terminations no Rust
//!   destructor survives.
//! - [`broker`] — the cross-process language-server broker. One hidden broker
//!   owns each `(server launch, repository root)` pair; editor windows connect
//!   over an authenticated loopback socket so several karet instances share one
//!   expensive server process. Its process-management skeleton is generic over
//!   [`broker::BrokerProtocol`], so another persistent daemon can reuse the
//!   election, endpoint and fanout machinery without inheriting LSP semantics.
//!
//! The composition root checks [`broker::requested`] / [`supervisor::requested`]
//! first thing in `main()` and hands off to the matching `run_from_env`.

pub mod broker;
pub mod supervisor;
