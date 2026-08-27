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
//! Reading is deliberately built on [`AsyncBufReadExt::fill_buf`] and a
//! [`FrameReader`] that owns its partial bytes, rather than on `read_exact`.
//! Both connection loops read inside a `tokio::select!`, where a future that
//! loses the race is dropped mid-flight: `read_exact` would take its
//! half-consumed bytes with it and every frame after that would be parsed from
//! the wrong offset. `fill_buf` is cancel-safe and the accumulator lives in the
//! reader rather than in the future, so losing the race costs nothing.
//!
//! Deliberately *not* [`karet_jsonrpc::Framing`]: that trait exists to carry
//! JSON-RPC envelopes, and this stream carries neither JSON nor RPC. The shape is
//! the same because the problem is.

use tokio::io::AsyncBufRead;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;

use super::RemoteError;

/// An uncompressed body.
const CODEC_RAW: u8 = 0;
/// A body compressed with raw deflate.
const CODEC_DEFLATE: u8 = 1;

/// The length prefix plus the codec tag.
const HEADER: usize = 5;

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

/// A framed reader that survives being cancelled.
///
/// Bytes pulled off the stream but not yet forming a whole frame live here, not
/// in the future returned by [`next`](Self::next). Dropping that future — which
/// is what `tokio::select!` does to every branch that loses — therefore loses no
/// bytes, and the next call resumes exactly where this one stopped.
pub(super) struct FrameReader<R> {
    reader: R,
    /// Bytes read from the stream and not yet consumed by a complete frame.
    pending: Vec<u8>,
}

impl<R> FrameReader<R>
where
    R: AsyncBufRead + Unpin,
{
    pub(super) fn new(reader: R) -> Self {
        Self {
            reader,
            pending: Vec::new(),
        }
    }

    /// Read the next frame's body, or `None` on a clean end of stream between
    /// frames.
    ///
    /// An end of stream *part way* through a frame is an error, not an ending: it
    /// means the peer died mid-message and whatever follows cannot be trusted.
    ///
    /// Cancel-safe: safe to use directly as a `tokio::select!` branch.
    pub(super) async fn next(&mut self) -> Result<Option<Vec<u8>>, RemoteError> {
        loop {
            if let Some(body) = self.take_frame()? {
                return Ok(Some(body));
            }
            // `fill_buf` is the cancel-safe primitive: it either yields bytes that
            // are still on the stream until `consume`, or nothing happened at all.
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                return if self.pending.is_empty() {
                    Ok(None)
                } else {
                    Err(RemoteError::Protocol(
                        "the stream ended part way through a frame".to_owned(),
                    ))
                };
            }
            let filled = available.len();
            self.pending.extend_from_slice(available);
            self.reader.consume(filled);
        }
    }

    /// Split one complete frame off the front of `pending`, if there is one.
    ///
    /// `None` means "not yet, read more" — never "malformed"; a length prefix
    /// that cannot be honoured is an error rather than a wait, because no amount
    /// of further reading would rescue it.
    fn take_frame(&mut self) -> Result<Option<Vec<u8>>, RemoteError> {
        let Some(header) = self.pending.get(..HEADER - 1) else {
            return Ok(None);
        };
        let len = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        if len == 0 {
            return Err(RemoteError::Protocol("frame has no codec tag".to_owned()));
        }
        if len > MAX_FRAME {
            return Err(RemoteError::Protocol(format!(
                "frame of {len} bytes exceeds the {MAX_FRAME}-byte cap"
            )));
        }
        // `len` counts the codec tag, so the whole frame is the 4-byte prefix
        // plus `len`.
        let total = HEADER - 1 + len;
        if self.pending.len() < total {
            return Ok(None);
        }
        let codec = self.pending[HEADER - 1];
        let payload = self.pending[HEADER..total].to_vec();
        self.pending.drain(..total);
        decode_body(codec, payload).map(Some)
    }
}

/// Undo whatever [`encode_body`] did.
fn decode_body(codec: u8, payload: Vec<u8>) -> Result<Vec<u8>, RemoteError> {
    use std::io::Read;

    match codec {
        CODEC_RAW => Ok(payload),
        CODEC_DEFLATE => {
            let mut body = Vec::new();
            flate2::read::DeflateDecoder::new(&payload[..])
                .take(MAX_FRAME as u64)
                .read_to_end(&mut body)
                .map_err(|error| RemoteError::Protocol(format!("corrupt frame: {error}")))?;
            Ok(body)
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
        let mut reader = FrameReader::new(tokio::io::BufReader::new(server));
        reader.next().await.ok()?
    }

    /// A reader over `bytes`, already at end of stream.
    fn reader_over(bytes: &[u8]) -> FrameReader<tokio::io::BufReader<&[u8]>> {
        FrameReader::new(tokio::io::BufReader::new(bytes))
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
        assert_eq!(reader_over(&[]).next().await.ok().flatten(), None);
    }

    /// Back-to-back frames in one buffer must both come out, in order: a reader
    /// that accumulates has to drain what it holds before reading more.
    #[tokio::test]
    async fn two_frames_delivered_together_are_read_in_order() {
        let mut stream = Vec::new();
        let _ = write(&mut stream, b"first").await;
        let _ = write(&mut stream, b"second").await;
        let mut reader = reader_over(&stream);

        assert_eq!(reader.next().await.ok().flatten(), Some(b"first".to_vec()));
        assert_eq!(reader.next().await.ok().flatten(), Some(b"second".to_vec()));
        assert_eq!(reader.next().await.ok().flatten(), None);
    }

    /// The property both connection loops depend on. A read that loses a
    /// `select!` race is dropped part way through a frame; the bytes it had
    /// already taken off the stream must survive in the reader, or every frame
    /// after this one is parsed from the wrong offset.
    #[tokio::test]
    async fn a_read_cancelled_mid_frame_keeps_the_bytes_it_took() {
        let body = b"a frame that arrives in two writes".to_vec();
        let (mut client, server) = tokio::io::duplex(1024);
        let mut reader = FrameReader::new(tokio::io::BufReader::new(server));
        // A raw frame, since the body is below `COMPRESS_ABOVE`.
        let len = u32::try_from(body.len() + 1).unwrap_or_default();
        let _ = client.write_all(&len.to_be_bytes()).await;
        let _ = client.write_all(&[CODEC_RAW]).await;
        let _ = client.write_all(&body[..8]).await;

        // Biased, so the read is polled first and consumes what has arrived
        // before the ready branch takes the race away from it.
        let mut lost = 0;
        for _ in 0..3 {
            tokio::select! {
                biased;
                frame = reader.next() => { let _ = frame; },
                () = std::future::ready(()) => lost += 1,
            }
        }
        assert_eq!(lost, 3, "the read must lose every race");

        let _ = client.write_all(&body[8..]).await;

        assert_eq!(reader.next().await.ok().flatten(), Some(body));
    }

    /// A peer that died mid-frame is not a clean ending — trusting the truncated
    /// bytes would desynchronize everything after them.
    #[tokio::test]
    async fn a_truncated_frame_is_an_error_not_an_ending() {
        // A header promising 32 bytes, followed by 4 and a close.
        let mut stream = 32_u32.to_be_bytes().to_vec();
        stream.push(CODEC_RAW);
        stream.extend_from_slice(b"only");

        assert!(reader_over(&stream).next().await.is_err());
    }

    /// A corrupted length prefix must fail rather than allocate whatever it said.
    #[tokio::test]
    async fn an_oversized_length_prefix_is_refused_before_allocating() {
        let mut stream = u32::try_from(MAX_FRAME + 1)
            .unwrap_or(u32::MAX)
            .to_be_bytes()
            .to_vec();
        stream.push(CODEC_RAW);

        assert!(reader_over(&stream).next().await.is_err());
    }

    #[tokio::test]
    async fn an_unknown_codec_tag_is_refused() {
        let mut stream = 2_u32.to_be_bytes().to_vec();
        stream.extend_from_slice(&[99, b'x']);

        assert!(reader_over(&stream).next().await.is_err());
    }

    #[tokio::test]
    async fn a_zero_length_frame_is_refused() {
        assert!(reader_over(&0_u32.to_be_bytes()).next().await.is_err());
    }
}
