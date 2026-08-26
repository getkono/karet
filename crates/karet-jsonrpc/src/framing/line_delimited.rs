//! Newline-delimited JSON framing (ACP, and JSON-RPC over stdio generally).
//!
//! One compact JSON document per line: no headers, no length prefix. Reading
//! accumulates bytes until a `\n`, strips an optional preceding `\r`, and skips
//! blank lines, so a peer that pads its stream stays in sync. The
//! [`MAX_MESSAGE_BYTES`] cap is enforced *incrementally* while accumulating, so a
//! peer that never sends a newline cannot grow memory without bound.

use std::future::Future;
use std::io;

use tokio::io::AsyncBufRead;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;

use super::Framing;
use super::MAX_MESSAGE_BYTES;

/// Framing failures while reading a line-delimited message.
#[derive(Debug, thiserror::Error)]
pub enum LineError {
    /// The underlying stream failed (including EOF mid-line).
    #[error("i/o error on the peer stream: {0}")]
    Io(#[from] io::Error),
    /// The line grew past [`MAX_MESSAGE_BYTES`] before a newline arrived.
    #[error("message line length exceeds the {MAX_MESSAGE_BYTES}-byte cap")]
    TooLarge,
}

/// Newline-delimited JSON bodies.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LineDelimited;

impl Framing for LineDelimited {
    type Error = LineError;

    fn read_frame<R>(
        reader: &mut R,
    ) -> impl Future<Output = Result<Option<Vec<u8>>, LineError>> + Send
    where
        R: AsyncBufRead + Send + Unpin,
    {
        read_frame(reader)
    }

    fn write_frame<W>(writer: &mut W, body: &[u8]) -> impl Future<Output = io::Result<()>> + Send
    where
        W: AsyncWrite + Send + Unpin,
    {
        write_frame(writer, body)
    }
}

/// Read one newline-delimited message body, or `None` on a clean EOF.
///
/// Blank lines are skipped rather than yielded as empty bodies. A final line
/// without a trailing newline is still returned. EOF with nothing buffered is a
/// clean end of stream (`Ok(None)`), not an error.
///
/// # Errors
///
/// Returns [`LineError::Io`] if the stream fails, or [`LineError::TooLarge`] if a
/// single line exceeds [`MAX_MESSAGE_BYTES`].
pub async fn read_frame<R>(reader: &mut R) -> Result<Option<Vec<u8>>, LineError>
where
    R: AsyncBufRead + Unpin,
{
    let mut line: Vec<u8> = Vec::new();
    loop {
        // `fill_buf`/`consume` rather than `read_until`: the cap has to be
        // enforced *while* accumulating, and `read_until` only returns once the
        // delimiter (or EOF) arrives.
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            // EOF. A part-read line is the last message; nothing buffered ends
            // the stream cleanly.
            return Ok(finish(std::mem::take(&mut line)));
        }
        match available.iter().position(|byte| *byte == b'\n') {
            Some(at) => {
                if line.len() + at > MAX_MESSAGE_BYTES {
                    return Err(LineError::TooLarge);
                }
                line.extend_from_slice(&available[..at]);
                reader.consume(at + 1);
                if let Some(body) = finish(std::mem::take(&mut line)) {
                    return Ok(Some(body));
                }
                // A blank line: keep reading for the next real message.
            },
            None => {
                let taken = available.len();
                if line.len() + taken > MAX_MESSAGE_BYTES {
                    return Err(LineError::TooLarge);
                }
                line.extend_from_slice(available);
                reader.consume(taken);
            },
        }
    }
}

/// Write `body` as one line and flush.
///
/// `body` must not contain a raw newline; compact `serde_json` output never
/// does, which is what makes this framing safe for JSON-RPC payloads.
///
/// # Errors
///
/// Returns the underlying [`io::Error`] if the write or flush fails.
pub async fn write_frame<W>(writer: &mut W, body: &[u8]) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    writer.write_all(body).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

/// Trim a trailing `\r` and drop the line entirely if nothing is left.
fn finish(mut line: Vec<u8>) -> Option<Vec<u8>> {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    (!line.is_empty()).then_some(line)
}

#[cfg(test)]
mod tests {
    use tokio::io::BufReader;
    use tokio::io::duplex;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

    #[tokio::test]
    async fn roundtrips_a_message() -> TestResult {
        let (reader, mut writer) = duplex(4096);
        write_frame(&mut writer, br#"{"jsonrpc":"2.0"}"#).await?;
        write_frame(&mut writer, b"second").await?;
        drop(writer);
        let mut reader = BufReader::new(reader);
        assert_eq!(
            read_frame(&mut reader).await?.as_deref(),
            Some(br#"{"jsonrpc":"2.0"}"#.as_slice())
        );
        assert_eq!(
            read_frame(&mut reader).await?.as_deref(),
            Some(b"second".as_slice())
        );
        assert!(read_frame(&mut reader).await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn survives_byte_at_a_time_delivery() -> TestResult {
        let (reader, mut writer) = duplex(4);
        let feeder = tokio::spawn(async move {
            for byte in b"payload\n" {
                if writer.write_all(&[*byte]).await.is_err() {
                    return;
                }
                let _ = writer.flush().await;
            }
        });
        let mut reader = BufReader::new(reader);
        assert_eq!(
            read_frame(&mut reader).await?.as_deref(),
            Some(b"payload".as_slice())
        );
        feeder.await?;
        Ok(())
    }

    #[tokio::test]
    async fn skips_blank_lines_and_tolerates_crlf() -> TestResult {
        let (reader, mut writer) = duplex(4096);
        writer
            .write_all(b"\n\r\n{\"a\":1}\r\n\n{\"b\":2}\n")
            .await?;
        drop(writer);
        let mut reader = BufReader::new(reader);
        assert_eq!(
            read_frame(&mut reader).await?.as_deref(),
            Some(br#"{"a":1}"#.as_slice())
        );
        assert_eq!(
            read_frame(&mut reader).await?.as_deref(),
            Some(br#"{"b":2}"#.as_slice())
        );
        assert!(read_frame(&mut reader).await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn returns_a_final_line_without_a_newline() -> TestResult {
        let (reader, mut writer) = duplex(4096);
        writer.write_all(b"tail").await?;
        drop(writer);
        let mut reader = BufReader::new(reader);
        assert_eq!(
            read_frame(&mut reader).await?.as_deref(),
            Some(b"tail".as_slice())
        );
        assert!(read_frame(&mut reader).await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn clean_eof_yields_none() -> TestResult {
        let (reader, writer) = duplex(4096);
        drop(writer);
        let mut reader = BufReader::new(reader);
        assert!(read_frame(&mut reader).await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn rejects_an_oversized_line() -> TestResult {
        let (reader, mut writer) = duplex(64 * 1024);
        let feeder = tokio::spawn(async move {
            let chunk = vec![b'x'; 64 * 1024];
            // Never sends a newline: the cap must trip while accumulating.
            loop {
                if writer.write_all(&chunk).await.is_err() {
                    return;
                }
            }
        });
        let mut reader = BufReader::new(reader);
        let err = read_frame(&mut reader).await;
        assert!(matches!(err, Err(LineError::TooLarge)));
        feeder.abort();
        Ok(())
    }
}
