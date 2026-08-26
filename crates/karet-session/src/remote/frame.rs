//! Length-prefixed frames over any byte stream.
//!
//! ```text
//! [u32 BE length][u8 codec: 0 = raw, 1 = deflate][body …]
//! ```
//!
//! The codec tag is per frame, so compression can start, stop or change without
//! the two ends renegotiating — a highlight payload compresses well and a
//! keystroke acknowledgement does not, and each pays only for itself.
//!
//! Deliberately *not* [`karet_jsonrpc::Framing`]: that trait exists to carry
//! JSON-RPC envelopes, and this stream carries neither JSON nor RPC. The shape is
//! the same because the problem is.

use tokio::io::AsyncBufRead;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;

use super::RemoteError;

/// An uncompressed body.
const CODEC_RAW: u8 = 0;
/// A body compressed with raw deflate.
const CODEC_DEFLATE: u8 = 1;

/// The largest frame that will be read or written.
///
/// A remote session ships whole file contents (a document on open, a PDF chunk),
/// so this is generous — but it is still a bound, because a corrupted length
/// prefix must fail rather than allocate whatever it happened to say.
pub(super) const MAX_FRAME: usize = 64 * 1024 * 1024;

/// Bodies at least this large are compressed before writing.
///
/// Below it, deflate's header plus the CPU cost outweigh anything saved. Above
/// it, the payload is derived data — highlight spans, document text — which is
/// repetitive enough to compress several-fold.
const COMPRESS_ABOVE: usize = 4 * 1024;

/// Write `body` as one frame and flush.
///
/// Compression is decided per frame and recorded in the tag, so a reader never
/// has to know what the writer chose.
pub(super) async fn write<W>(writer: &mut W, body: &[u8]) -> Result<(), RemoteError>
where
    W: AsyncWrite + Unpin,
{
    let (codec, payload) = encode_body(body);
    let len = u32::try_from(payload.len().saturating_add(1))
        .map_err(|_| RemoteError::Protocol("frame exceeds the length prefix".to_owned()))?;
    if payload.len() + 1 > MAX_FRAME {
        return Err(RemoteError::Protocol(
            "frame exceeds the size cap".to_owned(),
        ));
    }
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&[codec]).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

/// Compress `body` when it is large enough to be worth it.
fn encode_body(body: &[u8]) -> (u8, std::borrow::Cow<'_, [u8]>) {
    use std::io::Write;

    if body.len() < COMPRESS_ABOVE {
        return (CODEC_RAW, std::borrow::Cow::Borrowed(body));
    }
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    // A compressor that fails has nothing to say about correctness — fall back to
    // the raw body rather than failing a frame we can perfectly well send.
    if encoder.write_all(body).is_err() {
        return (CODEC_RAW, std::borrow::Cow::Borrowed(body));
    }
    match encoder.finish() {
        // Compression that did not help is not applied: a tiny incompressible
        // payload should not pay for a deflate header.
        Ok(packed) if packed.len() < body.len() => (CODEC_DEFLATE, std::borrow::Cow::Owned(packed)),
        _ => (CODEC_RAW, std::borrow::Cow::Borrowed(body)),
    }
}

/// Read one frame's body, or `None` on a clean end of stream between frames.
///
/// An end of stream *part way* through a frame is an error, not an ending: it
/// means the peer died mid-message and whatever follows cannot be trusted.
pub(super) async fn read<R>(reader: &mut R) -> Result<Option<Vec<u8>>, RemoteError>
where
    R: AsyncRead + AsyncBufRead + Unpin,
{
    let mut header = [0_u8; 4];
    match reader.read_exact(&mut header).await {
        Ok(_) => {},
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let len = u32::from_be_bytes(header) as usize;
    if len == 0 {
        return Err(RemoteError::Protocol("frame has no codec tag".to_owned()));
    }
    if len > MAX_FRAME {
        return Err(RemoteError::Protocol(format!(
            "frame of {len} bytes exceeds the {MAX_FRAME}-byte cap"
        )));
    }
    let mut codec = [0_u8; 1];
    reader.read_exact(&mut codec).await?;
    let mut payload = vec![0_u8; len - 1];
    reader.read_exact(&mut payload).await?;
    decode_body(codec[0], payload)
}

/// Undo whatever [`encode_body`] did.
fn decode_body(codec: u8, payload: Vec<u8>) -> Result<Option<Vec<u8>>, RemoteError> {
    use std::io::Read;

    match codec {
        CODEC_RAW => Ok(Some(payload)),
        CODEC_DEFLATE => {
            let mut body = Vec::new();
            flate2::read::DeflateDecoder::new(&payload[..])
                .take(MAX_FRAME as u64)
                .read_to_end(&mut body)
                .map_err(|error| RemoteError::Protocol(format!("corrupt frame: {error}")))?;
            Ok(Some(body))
        },
        other => Err(RemoteError::Protocol(format!(
            "unknown frame codec {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip `body` through a real duplex stream.
    async fn round_trip(body: &[u8]) -> Option<Vec<u8>> {
        let (mut client, server) = tokio::io::duplex(MAX_FRAME.min(1 << 20));
        write(&mut client, body).await.ok()?;
        drop(client);
        let mut reader = tokio::io::BufReader::new(server);
        read(&mut reader).await.ok()?
    }

    #[tokio::test]
    async fn a_small_body_round_trips_uncompressed() {
        let body = b"a keystroke acknowledgement".to_vec();

        assert_eq!(round_trip(&body).await, Some(body));
    }

    /// The whole point of the codec tag: a large repetitive payload — which is
    /// what highlight spans and document text are — must actually shrink.
    #[tokio::test]
    async fn a_large_repetitive_body_round_trips_compressed_and_smaller() {
        let body = "fn main() {}\n".repeat(4096).into_bytes();

        let (codec, payload) = encode_body(&body);

        assert_eq!(codec, CODEC_DEFLATE);
        assert!(payload.len() < body.len() / 4, "{} bytes", payload.len());
        assert_eq!(round_trip(&body).await, Some(body));
    }

    /// Random bytes do not compress. Applying deflate anyway would make the frame
    /// bigger, so the encoder must decline.
    #[test]
    fn an_incompressible_body_is_sent_raw() {
        let body: Vec<u8> = (0..8192_u32)
            .map(|i| i.wrapping_mul(2_654_435_761).to_le_bytes()[0])
            .collect();

        let (codec, payload) = encode_body(&body);

        if codec == CODEC_DEFLATE {
            assert!(payload.len() < body.len());
        } else {
            assert_eq!(payload.len(), body.len());
        }
    }

    #[tokio::test]
    async fn an_empty_body_round_trips() {
        assert_eq!(round_trip(b"").await, Some(Vec::new()));
    }

    /// A clean end of stream between frames is how a peer says goodbye.
    #[tokio::test]
    async fn a_closed_stream_reads_as_the_end() {
        let (client, server) = tokio::io::duplex(64);
        drop(client);
        let mut reader = tokio::io::BufReader::new(server);

        assert_eq!(read(&mut reader).await.ok().flatten(), None);
    }

    /// A peer that died mid-frame is not a clean ending — trusting the truncated
    /// bytes would desynchronize everything after them.
    #[tokio::test]
    async fn a_truncated_frame_is_an_error_not_an_ending() {
        let (mut client, server) = tokio::io::duplex(64);
        // A header promising 32 bytes, followed by 4 and a close.
        let _ = client.write_all(&32_u32.to_be_bytes()).await;
        let _ = client.write_all(&[CODEC_RAW]).await;
        let _ = client.write_all(b"only").await;
        drop(client);
        let mut reader = tokio::io::BufReader::new(server);

        assert!(read(&mut reader).await.is_err());
    }

    /// A corrupted length prefix must fail rather than allocate whatever it said.
    #[tokio::test]
    async fn an_oversized_length_prefix_is_refused_before_allocating() {
        let (mut client, server) = tokio::io::duplex(64);
        let _ = client
            .write_all(
                &u32::try_from(MAX_FRAME + 1)
                    .unwrap_or(u32::MAX)
                    .to_be_bytes(),
            )
            .await;
        let _ = client.write_all(&[CODEC_RAW]).await;
        drop(client);
        let mut reader = tokio::io::BufReader::new(server);

        assert!(read(&mut reader).await.is_err());
    }

    #[tokio::test]
    async fn an_unknown_codec_tag_is_refused() {
        let (mut client, server) = tokio::io::duplex(64);
        let _ = client.write_all(&2_u32.to_be_bytes()).await;
        let _ = client.write_all(&[99]).await;
        let _ = client.write_all(b"x").await;
        drop(client);
        let mut reader = tokio::io::BufReader::new(server);

        assert!(read(&mut reader).await.is_err());
    }

    #[tokio::test]
    async fn a_zero_length_frame_is_refused() {
        let (mut client, server) = tokio::io::duplex(64);
        let _ = client.write_all(&0_u32.to_be_bytes()).await;
        drop(client);
        let mut reader = tokio::io::BufReader::new(server);

        assert!(read(&mut reader).await.is_err());
    }
}
