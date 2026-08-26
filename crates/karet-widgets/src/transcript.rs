//! An append-only, wrapping, auto-scrolling conversation transcript.
//!
//! A live agent conversation is a stream: messages arrive one at a time, the last
//! one grows token by token, and the reader wants to stay at the tail unless they
//! deliberately scrolled away. [`Transcript`] is that surface — the piece
//! [`scroll::draw_scrollable_lines`](crate::scroll::draw_scrollable_lines) is not,
//! because that one paints a finished `Vec<Line>` at an offset with no wrapping,
//! no append semantics and no follow-the-tail.
//!
//! Three properties carry the widget:
//!
//! - **Incremental append.** Each message caches the styled rows it wrapped to,
//!   keyed by the width it wrapped at. Appending a message wraps *that* message;
//!   the earlier ones are reused verbatim. Only a width change (or a theme change,
//!   which the consumer signals with [`Transcript::invalidate`]) re-wraps the lot.
//! - **Stick-to-bottom.** The view follows the tail by default; scrolling up
//!   releases it, scrolling back to the bottom re-engages it. Appending while
//!   released never moves the viewport.
//! - **A post-wrap extent.** [`Transcript::extent`] counts *wrapped rows*, not
//!   source lines, so the scrollbar thumb is exact on heavily wrapped content
//!   rather than optimistic.
//!
//! ## The reservation invariant
//!
//! Inherited verbatim from [`scroll`](crate::scroll): *reservation depends only on
//! the area and on which axes the view scrolls — never on the content extent or the
//! current offset. That is load-bearing: a content-dependent reservation feeds back
//! on itself, because reserving a column narrows the wrap width, which produces
//! more visual rows, which overflows the viewport, which reserves a column.*
//!
//! [`Transcript::paint`] therefore calls
//! [`reserve_tracks`](crate::scroll::reserve_tracks) **first** and derives the wrap
//! width from the content rect it hands back. The row count is computed after that
//! and never feeds back into the reservation.
//!
//! Message bodies are plain text by default; with the `markdown` feature a body may
//! be [`TranscriptBody::Markdown`], rendered through `karet-markdown` (headings,
//! lists, tables, syntax-highlighted fences) exactly as the hover popup and the
//! dialog body are.

use karet_core::ThemeRole;
use karet_theme::Theme;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::Line;

use crate::scroll::PaintedTracks;
use crate::scroll::ScrollAxes;
use crate::scroll::ScrollExtent;
use crate::scroll::ScrollbarStyles;
use crate::scroll::reserve_tracks;
use crate::text;

/// A transcript message's body, and how it should be rendered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptBody {
    /// Plain text, soft-wrapped at whitespace to the transcript's content width.
    Plain(String),
    /// Markdown, parsed and painted through the theme — available with the
    /// `markdown` feature.
    #[cfg(feature = "markdown")]
    Markdown(String),
}

impl TranscriptBody {
    /// The body's source text, whatever its rendering.
    #[must_use]
    pub fn source(&self) -> &str {
        match self {
            Self::Plain(text) => text,
            #[cfg(feature = "markdown")]
            Self::Markdown(text) => text,
        }
    }

    /// Whether there is nothing to paint (an empty body occupies no rows).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.source().is_empty()
    }

    /// Append `text` to the body's source — the streaming case, where a reply
    /// arrives token by token.
    pub fn push_str(&mut self, text: &str) {
        match self {
            Self::Plain(source) => source.push_str(text),
            #[cfg(feature = "markdown")]
            Self::Markdown(source) => source.push_str(text),
        }
    }
}

impl From<String> for TranscriptBody {
    fn from(text: String) -> Self {
        Self::Plain(text)
    }
}

impl From<&str> for TranscriptBody {
    fn from(text: &str) -> Self {
        Self::Plain(text.to_owned())
    }
}

/// One message of a transcript: an optional header line and a body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptMessage {
    /// The header painted above the body (an author, a role, a timestamp).
    /// Empty for a message that is only a body.
    pub header: String,
    /// The message text.
    pub body: TranscriptBody,
    /// The theme role the header is painted in, so a consumer can tell speakers
    /// apart without the widget knowing what a speaker is.
    pub accent: ThemeRole,
}

impl TranscriptMessage {
    /// A message headed `header` carrying `body`, accented as
    /// [`ThemeRole::DiagnosticInfo`].
    #[must_use]
    pub fn new(header: impl Into<String>, body: impl Into<TranscriptBody>) -> Self {
        Self {
            header: header.into(),
            body: body.into(),
            accent: ThemeRole::DiagnosticInfo,
        }
    }

    /// The same message, headed in `accent` instead.
    #[must_use]
    pub fn with_accent(mut self, accent: ThemeRole) -> Self {
        self.accent = accent;
        self
    }
}

/// A message plus the rows it last wrapped to, and the width they were wrapped at.
#[derive(Clone, Debug)]
struct Entry {
    message: TranscriptMessage,
    lines: Vec<Line<'static>>,
    /// The wrap width `lines` is valid for; `None` means "not wrapped yet".
    width: Option<u16>,
}

/// An append-only conversation view that wraps, caches, and sticks to the bottom.
///
/// The consumer owns the instance between frames: [`paint`](Self::paint) records
/// the viewport height and the settled offset that the scrolling methods and
/// [`extent`](Self::extent) then work from.
#[derive(Clone, Debug, Default)]
pub struct Transcript {
    entries: Vec<Entry>,
    /// Total wrapped rows, as of the last wrap pass.
    rows: usize,
    /// The first visible wrapped row.
    offset: usize,
    /// Whether the view is pinned to the tail.
    following: bool,
    /// The content height of the last paint, in rows.
    viewport: usize,
    /// How many message wraps have been performed — a cache-effectiveness
    /// diagnostic, and what the tests assert reuse against.
    wraps: usize,
}

impl Transcript {
    /// An empty transcript, following the tail.
    #[must_use]
    pub fn new() -> Self {
        Self {
            following: true,
            ..Self::default()
        }
    }

    /// Append `message` to the end of the transcript.
    ///
    /// Nothing earlier is re-wrapped: the new message is wrapped on the next
    /// paint and the cached rows of every message before it are reused.
    pub fn push(&mut self, message: TranscriptMessage) {
        self.entries.push(Entry {
            message,
            lines: Vec::new(),
            width: None,
        });
    }

    /// Replace the last message's body — the coarse form of the streaming case,
    /// for a reply that is re-rendered rather than extended.
    ///
    /// Returns `false` when the transcript is empty. Only the tail is
    /// invalidated.
    pub fn amend_tail(&mut self, body: impl Into<TranscriptBody>) -> bool {
        let Some(entry) = self.entries.last_mut() else {
            return false;
        };
        entry.message.body = body.into();
        entry.width = None;
        true
    }

    /// Append `text` to the last message's body — the token-by-token streaming
    /// case. Returns `false` when the transcript is empty.
    pub fn extend_tail(&mut self, text: &str) -> bool {
        let Some(entry) = self.entries.last_mut() else {
            return false;
        };
        entry.message.body.push_str(text);
        entry.width = None;
        true
    }

    /// Drop every message and return to following the tail.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.rows = 0;
        self.offset = 0;
        self.following = true;
    }

    /// Drop every cached wrap, forcing a full re-wrap on the next paint.
    ///
    /// The cache is keyed by width alone, so this is how a consumer signals the
    /// one other input the rows depend on: the theme.
    pub fn invalidate(&mut self) {
        for entry in &mut self.entries {
            entry.width = None;
        }
    }

    /// How many messages the transcript holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the transcript holds no messages.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The message at `index`, or `None` when it is out of range.
    #[must_use]
    pub fn message(&self, index: usize) -> Option<&TranscriptMessage> {
        self.entries.get(index).map(|entry| &entry.message)
    }

    /// How many wrapped rows the message at `index` currently occupies — `0`
    /// when it is out of range or has not been wrapped yet.
    #[must_use]
    pub fn message_rows(&self, index: usize) -> usize {
        self.entries.get(index).map_or(0, |entry| {
            if entry.width.is_some() {
                entry.lines.len()
            } else {
                0
            }
        })
    }

    /// The total number of wrapped rows, as of the last wrap pass.
    #[must_use]
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// The first visible wrapped row.
    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// The content height of the last paint, in rows.
    #[must_use]
    pub fn viewport(&self) -> usize {
        self.viewport
    }

    /// Whether the view is pinned to the tail.
    #[must_use]
    pub fn is_following(&self) -> bool {
        self.following
    }

    /// How many message wraps have run since construction.
    ///
    /// A diagnostic on the cache: it rises by one per message actually wrapped, so
    /// appending to a settled transcript rises by exactly one, and a width change
    /// rises by the message count.
    #[must_use]
    pub fn wrap_count(&self) -> usize {
        self.wraps
    }

    /// The post-wrap vertical extent: wrapped rows, the current offset, and the
    /// last painted viewport.
    ///
    /// Unlike a line-counting extent this is exact, because the rows it counts are
    /// the rows that were painted.
    #[must_use]
    pub fn extent(&self) -> ScrollExtent {
        ScrollExtent::new(self.rows, self.offset, self.viewport)
    }

    /// The largest valid offset — the one that rests the last row against the
    /// bottom of the viewport.
    #[must_use]
    pub fn max_offset(&self) -> usize {
        self.rows.saturating_sub(self.viewport)
    }

    /// Scroll to `offset`, clamped, engaging the follow when it lands at the
    /// bottom and releasing it anywhere above.
    pub fn set_offset(&mut self, offset: usize) {
        let max = self.max_offset();
        self.offset = offset.min(max);
        self.following = self.offset >= max;
    }

    /// Scroll by `delta` rows — negative towards the start.
    pub fn scroll_by(&mut self, delta: i32) {
        let next = if delta < 0 {
            self.offset.saturating_sub(delta.unsigned_abs() as usize)
        } else {
            self.offset.saturating_add(delta as usize)
        };
        self.set_offset(next);
    }

    /// Scroll by `pages` viewports — negative towards the start.
    pub fn page_by(&mut self, pages: i32) {
        let rows = i32::try_from(self.viewport.max(1)).unwrap_or(i32::MAX);
        self.scroll_by(pages.saturating_mul(rows));
    }

    /// Jump to the first row, releasing the follow unless everything fits.
    pub fn scroll_to_top(&mut self) {
        self.set_offset(0);
    }

    /// Jump to the tail and re-engage the follow.
    pub fn scroll_to_bottom(&mut self) {
        self.offset = self.max_offset();
        self.following = true;
    }

    /// Paint the transcript into `area` of `buf`, returning the scrollbar track it
    /// reserved so a caller can hit-test the bar.
    ///
    /// The reservation runs first and the wrap width comes out of the content rect
    /// it leaves — see the module docs for why that order is load-bearing.
    pub fn paint(&mut self, buf: &mut Buffer, theme: &Theme, area: Rect) -> PaintedTracks {
        // Invariant: area and axes only. Nothing about the content may influence
        // this call, or the wrap width would feed back into the reservation.
        let (content, tracks) = reserve_tracks(area, ScrollAxes::VERTICAL);
        if content.width == 0 || content.height == 0 {
            return PaintedTracks::default();
        }
        self.viewport = usize::from(content.height);
        self.wrap_to(content.width, theme);
        self.settle();
        let end = self.offset.saturating_add(self.viewport);
        let mut row = 0usize;
        let mut y = content.y;
        'rows: for entry in &self.entries {
            let next = row.saturating_add(entry.lines.len());
            if next <= self.offset {
                row = next;
                continue;
            }
            for line in &entry.lines {
                if row >= end {
                    break 'rows;
                }
                if row >= self.offset {
                    buf.set_line(content.x, y, line, content.width);
                    y = y.saturating_add(1);
                }
                row = row.saturating_add(1);
            }
        }
        tracks.paint(
            buf,
            ScrollbarStyles::from_theme(theme),
            self.extent(),
            ScrollExtent::default(),
        )
    }

    /// [`paint`](Self::paint) against a frame's buffer.
    pub fn draw(&mut self, f: &mut Frame, theme: &Theme, area: Rect) -> PaintedTracks {
        self.paint(f.buffer_mut(), theme, area)
    }

    /// Wrap every message whose cache is not already valid for `width`, and total
    /// the rows.
    fn wrap_to(&mut self, width: u16, theme: &Theme) {
        let mut wraps = 0usize;
        let mut rows = 0usize;
        for (index, entry) in self.entries.iter_mut().enumerate() {
            if entry.width != Some(width) {
                entry.lines = wrap_message(&entry.message, index > 0, width, theme);
                entry.width = Some(width);
                wraps = wraps.saturating_add(1);
            }
            rows = rows.saturating_add(entry.lines.len());
        }
        self.wraps = self.wraps.saturating_add(wraps);
        self.rows = rows;
    }

    /// Pin the offset to the tail while following, and clamp it otherwise — so a
    /// released view holds still no matter how much arrives beneath it.
    fn settle(&mut self) {
        let max = self.max_offset();
        if self.following {
            self.offset = max;
        } else {
            self.offset = self.offset.min(max);
        }
    }
}

/// The styled rows one message wraps to at `width`, preceded by a blank spacer
/// row for every message but the first.
///
/// The spacer leads rather than trails so the tail of the transcript is the last
/// line of text, not an empty row below it.
fn wrap_message(
    message: &TranscriptMessage,
    spacer: bool,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    // Only a markdown body consults the theme for its own tokens; a plain one
    // uses it solely for the header accent.
    let mut lines = Vec::new();
    if spacer {
        lines.push(Line::default());
    }
    if !message.header.is_empty() {
        let style = theme.style(message.accent).add_modifier(Modifier::BOLD);
        lines.extend(
            text::wrap(&message.header, usize::from(width))
                .into_iter()
                .map(|row| Line::styled(row, style)),
        );
    }
    if message.body.is_empty() {
        return lines;
    }
    match &message.body {
        TranscriptBody::Plain(source) => lines.extend(
            text::wrap(source, usize::from(width))
                .into_iter()
                .map(Line::from),
        ),
        #[cfg(feature = "markdown")]
        TranscriptBody::Markdown(source) => {
            let doc = karet_markdown::parse(source).wrap(width);
            lines.extend(karet_markdown::view::to_ratatui(&doc, theme));
        },
    }
    lines
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
