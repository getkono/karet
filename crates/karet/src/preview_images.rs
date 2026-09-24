//! Local images for the markdown preview: resolved, sized, decoded off the UI thread,
//! and cached.
//!
//! The preview's policy, in one place:
//!
//! - only a relative path resolving to a regular file inside the workspace is loaded
//!   (the same trust boundary [`crate::links::resolve`] draws for links) — a remote
//!   URL, an absolute path, or anything outside the workspace stays a chip, and no
//!   network request is ever made;
//! - the draw path reads at most [`PROBE_BYTES`] of a file, for its header, so layout
//!   can reserve the image's rows before any pixel is decoded;
//! - an image is decoded only once it is painted — layout sizes every image in the
//!   document, but decodes none (bar those whose size only a decode can tell);
//! - decoding runs on one background thread and lands through the event loop, which
//!   repaints; files over the size guard or the pixel cap stay chips, and a file that
//!   has since become a symlink is refused, not followed;
//! - decoded pixels are budgeted, oldest-first, and an evicted image reloads on sight;
//!   the images on screen are never evicted, so a screen over budget runs over it.
//!
//! Built without the `images` feature, nothing is ever sized, so every image is a chip.

#[cfg(feature = "images")]
pub(crate) mod cache;
#[cfg(all(test, feature = "images"))]
pub(crate) mod tests;

#[cfg(feature = "images")]
pub(crate) use cache::Decoded;
#[cfg(feature = "images")]
pub(crate) use cache::Lookup;
#[cfg(feature = "images")]
pub(crate) use cache::PreviewImages;

/// The images a lean build cannot decode: every lookup misses.
#[cfg(not(feature = "images"))]
#[derive(Debug, Default)]
pub(crate) struct PreviewImages;

#[cfg(not(feature = "images"))]
impl PreviewImages {
    /// Sizes nothing, so the preview renders every image as a chip.
    pub(crate) fn sizer(&self, _source: &std::path::Path, _root: &std::path::Path) -> NoImages {
        NoImages
    }

    /// Nothing is ever loading.
    pub(crate) fn pendings(&self) -> Vec<crate::app::Pending> {
        Vec::new()
    }

    /// Layout never changes under a lean build.
    pub(crate) fn generation(&self) -> u64 {
        0
    }

    /// Nothing is ever decoded, so nothing is evicted.
    pub(crate) fn end_frame(&self) {}
}

/// Sizes nothing.
#[cfg(not(feature = "images"))]
pub(crate) struct NoImages;

#[cfg(not(feature = "images"))]
impl karet_markdown::ImageSizer for NoImages {
    fn dimensions(&self, _image: &karet_markdown::ImageRef) -> Option<(u32, u32)> {
        None
    }
}

/// How much of a file the draw path reads to find its size.
#[cfg(feature = "images")]
const PROBE_BYTES: u64 = 64 * 1024;
