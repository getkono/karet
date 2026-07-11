//! `Content-Length`-framed message codec (the LSP "base protocol").
//!
//! Every LSP message travels as an HTTP-style header block terminated by a blank
//! line, followed by exactly `Content-Length` bytes of JSON. This module frames
//! and de-frames those messages over any [`AsyncBufRead`]/[`AsyncWrite`] pair, so
//! the transport is testable in-memory (`tokio::io::duplex`) and the process layer
//! stays a thin wrapper around child stdio.

use std::io;

use tokio::io::AsyncBufRead;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;

/// The largest message body we will read, guarding against a corrupt or hostile
/// `Content-Length` allocating unbounded memory.
pub(crate) const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

/// Framing failures while reading a message.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CodecError {
    /// The underlying stream failed (including EOF mid-message).
    #[error("i/o error on the language-server stream: {0}")]
    Io(#[from] io::Error),
    /// The header block ended without a `Content-Length` header.
    #[error("message headers carried no Content-Length")]
    MissingContentLength,
    /// A header line was not valid `Name: value` ASCII or its value was bad.
    #[error("malformed message header: {0:?}")]
    MalformedHeader(String),
    /// The declared body length exceeds [`MAX_MESSAGE_BYTES`].
    #[error("declared message length {0} exceeds the {MAX_MESSAGE_BYTES}-byte cap")]
    TooLarge(usize),
}

/// Read one framed message body, or `None` on a clean EOF between messages.
///
/// Headers are parsed case-insensitively; unknown headers (e.g. `Content-Type`)
/// are ignored. Both `\r\n` and bare `\n` line endings are accepted. EOF in the
/// middle of a message (headers or body) is a [`CodecError::Io`] error.
pub(crate) async fn read_frame<R>(reader: &mut R) -> Result<Option<Vec<u8>>, CodecError>
where
    R: AsyncBufRead + Unpin,
{
    let mut content_length: Option<usize> = None;
    let mut line: Vec<u8> = Vec::new();
    let mut read_any = false;
    loop {
        line.clear();
        let n = reader.read_until(b'\n', &mut line).await?;
        if n == 0 {
            if read_any {
                // EOF after we already consumed part of a message.
                return Err(CodecError::Io(io::ErrorKind::UnexpectedEof.into()));
            }
            return Ok(None);
        }
        read_any = true;
        let trimmed = trim_line_ending(&line);
        if trimmed.is_empty() {
            break; // end of the header block
        }
        let text = std::str::from_utf8(trimmed)
            .map_err(|_| CodecError::MalformedHeader(String::from_utf8_lossy(trimmed).into()))?;
        let Some((name, value)) = text.split_once(':') else {
            return Err(CodecError::MalformedHeader(text.to_owned()));
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            let len: usize = value
                .trim()
                .parse()
                .map_err(|_| CodecError::MalformedHeader(text.to_owned()))?;
            if len > MAX_MESSAGE_BYTES {
                return Err(CodecError::TooLarge(len));
            }
            content_length = Some(len);
        }
        // Other headers (Content-Type, …) are permitted and ignored.
    }
    let len = content_length.ok_or(CodecError::MissingContentLength)?;
    let mut body = vec![0_u8; len];
    reader.read_exact(&mut body).await?;
    Ok(Some(body))
}

/// Write `body` as one framed message and flush.
pub(crate) async fn write_frame<W>(writer: &mut W, body: &[u8]) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(body).await?;
    writer.flush().await
}

/// Strip one trailing `\r\n` or `\n` from a header line.
fn trim_line_ending(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
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
        let mut reader = BufReader::new(reader);
        let body = read_frame(&mut reader).await?;
        assert_eq!(body.as_deref(), Some(br#"{"jsonrpc":"2.0"}"#.as_slice()));
        Ok(())
    }

    #[tokio::test]
    async fn survives_byte_at_a_time_delivery() -> TestResult {
        let (reader, mut writer) = duplex(16);
        let frame = b"Content-Length: 7\r\nContent-Type: application/vscode-jsonrpc\r\n\r\npayload";
        let feeder = tokio::spawn(async move {
            for byte in frame {
                if writer.write_all(&[*byte]).await.is_err() {
                    return;
                }
                let _ = writer.flush().await;
            }
        });
        let mut reader = BufReader::new(reader);
        let body = read_frame(&mut reader).await?;
        assert_eq!(body.as_deref(), Some(b"payload".as_slice()));
        feeder.await?;
        Ok(())
    }

    #[tokio::test]
    async fn roundtrips_a_huge_payload() -> TestResult {
        let payload = vec![b'x'; 3 * 1024 * 1024];
        let expected = payload.clone();
        let (reader, mut writer) = duplex(64 * 1024);
        let feeder = tokio::spawn(async move {
            let _ = write_frame(&mut writer, &payload).await;
        });
        let mut reader = BufReader::new(reader);
        let body = read_frame(&mut reader).await?;
        assert_eq!(body, Some(expected));
        feeder.await?;
        Ok(())
    }

    #[tokio::test]
    async fn accepts_lf_only_and_case_insensitive_headers() -> TestResult {
        let (reader, mut writer) = duplex(4096);
        writer.write_all(b"CONTENT-length: 2\n\nok").await?;
        writer.flush().await?;
        let mut reader = BufReader::new(reader);
        let body = read_frame(&mut reader).await?;
        assert_eq!(body.as_deref(), Some(b"ok".as_slice()));
        Ok(())
    }

    #[tokio::test]
    async fn rejects_missing_content_length() -> TestResult {
        let (reader, mut writer) = duplex(4096);
        writer
            .write_all(b"Content-Type: application/json\r\n\r\n{}")
            .await?;
        writer.flush().await?;
        let mut reader = BufReader::new(reader);
        let err = read_frame(&mut reader).await;
        assert!(matches!(err, Err(CodecError::MissingContentLength)));
        Ok(())
    }

    #[tokio::test]
    async fn rejects_non_numeric_length_and_headerless_line() -> TestResult {
        for garbage in [
            b"Content-Length: abc\r\n\r\n".as_slice(),
            b"not a header line\r\n\r\n".as_slice(),
        ] {
            let (reader, mut writer) = duplex(4096);
            writer.write_all(garbage).await?;
            writer.flush().await?;
            let mut reader = BufReader::new(reader);
            let err = read_frame(&mut reader).await;
            assert!(matches!(err, Err(CodecError::MalformedHeader(_))));
        }
        Ok(())
    }

    #[tokio::test]
    async fn rejects_oversized_declared_length() -> TestResult {
        let (reader, mut writer) = duplex(4096);
        let header = format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE_BYTES + 1);
        writer.write_all(header.as_bytes()).await?;
        writer.flush().await?;
        let mut reader = BufReader::new(reader);
        let err = read_frame(&mut reader).await;
        assert!(matches!(err, Err(CodecError::TooLarge(_))));
        Ok(())
    }

    #[tokio::test]
    async fn clean_eof_yields_none() -> TestResult {
        let (reader, writer) = duplex(4096);
        drop(writer); // no bytes ever written
        let mut reader = BufReader::new(reader);
        assert!(read_frame(&mut reader).await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn eof_mid_headers_and_mid_body_error() -> TestResult {
        for partial in [
            b"Content-Length: 10\r\n".as_slice(), // headers never finish
            b"Content-Length: 10\r\n\r\nabc".as_slice(), // body cut short
        ] {
            let (reader, mut writer) = duplex(4096);
            writer.write_all(partial).await?;
            writer.flush().await?;
            drop(writer);
            let mut reader = BufReader::new(reader);
            let err = read_frame(&mut reader).await;
            assert!(matches!(err, Err(CodecError::Io(_))));
        }
        Ok(())
    }

    #[tokio::test]
    async fn reads_back_to_back_messages() -> TestResult {
        let (reader, mut writer) = duplex(4096);
        write_frame(&mut writer, b"one").await?;
        write_frame(&mut writer, b"two").await?;
        drop(writer);
        let mut reader = BufReader::new(reader);
        assert_eq!(
            read_frame(&mut reader).await?.as_deref(),
            Some(b"one".as_slice())
        );
        assert_eq!(
            read_frame(&mut reader).await?.as_deref(),
            Some(b"two".as_slice())
        );
        assert!(read_frame(&mut reader).await?.is_none());
        Ok(())
    }
}
