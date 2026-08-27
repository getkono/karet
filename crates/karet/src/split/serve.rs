//! The backend half: serve this workspace and never draw anything.
//!
//! Everything the editor needs stays here — documents, git, language servers,
//! tree-sitter, the files. What leaves is edits and the derived data a renderer
//! needs, which is the whole reason the split is worth making.

use std::path::PathBuf;

use color_eyre::eyre::eyre;

use crate::split::kmux;

/// Serve `root` over stdin/stdout until the client disconnects.
///
/// Nothing is written to stdout but the protocol: the stream *is* stdout, so a
/// stray `println!` would corrupt the session. Diagnostics go to the log file,
/// which is why logging is initialized before this runs.
pub(crate) fn run_stdio(config: karet_session::session::SessionConfig) -> color_eyre::Result<()> {
    let runtime =
        tokio::runtime::Runtime::new().map_err(|error| eyre!("tokio runtime: {error}"))?;
    runtime.block_on(async move {
        let reader = tokio::io::BufReader::new(tokio::io::stdin());
        karet_session::remote::serve(config, reader, tokio::io::stdout())
            .await
            .map_err(|error| eyre!("serve: {error}"))
    })
}

/// Serve `root` over a channel the multiplexer forwards to the client it hosts.
///
/// Identical to [`run_stdio`] but for where the bytes come from — which is the
/// property the whole design rests on.
pub(crate) fn run_forwarded(
    config: karet_session::session::SessionConfig,
    channel: &kmux::Channel,
) -> color_eyre::Result<()> {
    let runtime =
        tokio::runtime::Runtime::new().map_err(|error| eyre!("tokio runtime: {error}"))?;
    let endpoint: PathBuf = channel.endpoint.clone();
    runtime.block_on(async move {
        let stream = tokio::net::UnixStream::connect(&endpoint)
            .await
            .map_err(|error| eyre!("connecting the forwarded channel: {error}"))?;
        let (reader, writer) = tokio::io::split(stream);
        karet_session::remote::serve(config, tokio::io::BufReader::new(reader), writer)
            .await
            .map_err(|error| eyre!("serve: {error}"))
    })
}
