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
    /// The underlying stream failed. EOF is *not* a failure here: a part-read
    /// final line is returned as the last message.
    #[error("i/o error on the peer stream: {0}")]
    Io(#[from] io::Error),
    /// The line grew past [`MAX_MESSAGE_BYTES`] before a newline arrived.
    #[error("message line length exceeds the {MAX_MESSAGE_BYTES}-byte cap")]
    TooLarge,
}

/// Newline-delimited JSON bodies.
///
/// This framing's answer to the trailing-partial-frame question
/// [`Framing::read_frame`] leaves open: an unterminated final line is a **final
/// message**, not an error. A newline is a terminator this framing cannot
/// distinguish from a peer that exited immediately after its last write, and
/// losing that message is the worse failure, so it is returned.
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
/// Blank lines are skipped rather than yielded as empty bodies. EOF with nothing
/// buffered is a clean end of stream (`Ok(None)`), not an error.
///
/// A final line **without** a trailing newline is returned as a message rather
/// than rejected as truncation — this framing's choice on the point
/// [`Framing::read_frame`] leaves to each implementation, so a peer that exits
/// right after its last write does not lose it.
///
/// # Errors
///
/// Returns [`LineError::Io`] if the stream fails, or [`LineError::TooLarge`] if a
/// single line exceeds [`MAX_MESSAGE_BYTES`].
pub async fn read_frame<R>(reader: &mut R) -> Result<Option<Vec<u8>>, LineError>
where
    R: AsyncBufRead + Unpin,
{
    read_frame_capped(reader, MAX_MESSAGE_BYTES).await
}

/// [`read_frame`]'s body, with the line cap as a parameter.
///
/// Only [`read_frame`] (at [`MAX_MESSAGE_BYTES`]) and the cap test call this:
/// tripping [`LineError::TooLarge`] over the real cap would mean pushing 64 MiB
/// through an 8 KiB buffer on every test run, and the code path is identical at
/// any `cap`.
async fn read_frame_capped<R>(reader: &mut R, cap: usize) -> Result<Option<Vec<u8>>, LineError>
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
                if line.len() + at > cap {
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
                if line.len() + taken > cap {
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
        const CAP: usize = 4 * 1024;
        let (reader, mut writer) = duplex(64 * 1024);
        let feeder = tokio::spawn(async move {
            let chunk = vec![b'x'; 8 * 1024];
            // Never sends a newline: the cap must trip while accumulating.
            loop {
                if writer.write_all(&chunk).await.is_err() {
                    return;
                }
            }
        });
        let mut reader = BufReader::new(reader);
        // The capped entry point, not `read_frame`: same code path, without
        // pushing 64 MiB through the buffer on every test run.
        let err = read_frame_capped(&mut reader, CAP).await;
        assert!(matches!(err, Err(LineError::TooLarge)));
        feeder.abort();
        Ok(())
    }

    #[tokio::test]
    async fn caps_a_terminated_line_too() -> TestResult {
        let (reader, mut writer) = duplex(4096);
        // A line that *does* end in a newline, but past the cap.
        writer.write_all(&[b'x'; 64]).await?;
        writer.write_all(b"\n").await?;
        drop(writer);
        let mut reader = BufReader::new(reader);
        let err = read_frame_capped(&mut reader, 16).await;
        assert!(matches!(err, Err(LineError::TooLarge)));
        Ok(())
    }

    /// A reader whose every poll fails, so [`LineError::Io`] is reachable.
    struct FailingReader;

    impl tokio::io::AsyncRead for FailingReader {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            std::task::Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "boom")))
        }
    }

    #[tokio::test]
    async fn surfaces_a_stream_failure_as_io() -> TestResult {
        let mut reader = BufReader::new(FailingReader);
        let Err(LineError::Io(error)) = read_frame(&mut reader).await else {
            return Err("expected an i/o error".into());
        };
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        Ok(())
    }
}
