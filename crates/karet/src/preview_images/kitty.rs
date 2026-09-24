//! Kitty unicode placeholders: a preview image at full resolution, drawn by text.
//!
//! The image's pixels are transmitted to the terminal once, under an id, and given a
//! *virtual* placement of `cols`×`rows` cells. Each cell of it is then an ordinary
//! character in ratatui's buffer — [`PLACEHOLDER`] followed by two diacritics naming
//! the cell's row and column, coloured with the image id (foreground) and the
//! placement id (underline colour) — which the terminal replaces with that cell of
//! the image. So an image scrolls, clips to its pane, and sits under a popup exactly
//! as text does, and nothing is re-sent when it moves.
//!
//! See <https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders>.

use std::io::Write as _;
use std::num::NonZeroU16;

use base64::Engine as _;
use karet_fileview::image::Image;
use ratatui::buffer::Buffer;
use ratatui::buffer::CellDiffOption;
use ratatui::layout::Rect;
use ratatui::style::Color;

/// The character a terminal replaces with a cell of a virtually placed image.
pub(crate) const PLACEHOLDER: char = '\u{10EEEE}';

/// The most base64 bytes one escape carries.
const CHUNK: usize = 4096;

/// The combining marks numbering a placeholder cell's row and column, in order: the
/// `n`th mark means `n`. Kitty's `rowcolumn-diacritics.txt`, verbatim.
pub(crate) const DIACRITICS: [char; 297] = [
    '\u{0305}',
    '\u{030D}',
    '\u{030E}',
    '\u{0310}',
    '\u{0312}',
    '\u{033D}',
    '\u{033E}',
    '\u{033F}',
    '\u{0346}',
    '\u{034A}',
    '\u{034B}',
    '\u{034C}',
    '\u{0350}',
    '\u{0351}',
    '\u{0352}',
    '\u{0357}',
    '\u{035B}',
    '\u{0363}',
    '\u{0364}',
    '\u{0365}',
    '\u{0366}',
    '\u{0367}',
    '\u{0368}',
    '\u{0369}',
    '\u{036A}',
    '\u{036B}',
    '\u{036C}',
    '\u{036D}',
    '\u{036E}',
    '\u{036F}',
    '\u{0483}',
    '\u{0484}',
    '\u{0485}',
    '\u{0486}',
    '\u{0487}',
    '\u{0592}',
    '\u{0593}',
    '\u{0594}',
    '\u{0595}',
    '\u{0597}',
    '\u{0598}',
    '\u{0599}',
    '\u{059C}',
    '\u{059D}',
    '\u{059E}',
    '\u{059F}',
    '\u{05A0}',
    '\u{05A1}',
    '\u{05A8}',
    '\u{05A9}',
    '\u{05AB}',
    '\u{05AC}',
    '\u{05AF}',
    '\u{05C4}',
    '\u{0610}',
    '\u{0611}',
    '\u{0612}',
    '\u{0613}',
    '\u{0614}',
    '\u{0615}',
    '\u{0616}',
    '\u{0617}',
    '\u{0657}',
    '\u{0658}',
    '\u{0659}',
    '\u{065A}',
    '\u{065B}',
    '\u{065D}',
    '\u{065E}',
    '\u{06D6}',
    '\u{06D7}',
    '\u{06D8}',
    '\u{06D9}',
    '\u{06DA}',
    '\u{06DB}',
    '\u{06DC}',
    '\u{06DF}',
    '\u{06E0}',
    '\u{06E1}',
    '\u{06E2}',
    '\u{06E4}',
    '\u{06E7}',
    '\u{06E8}',
    '\u{06EB}',
    '\u{06EC}',
    '\u{0730}',
    '\u{0732}',
    '\u{0733}',
    '\u{0735}',
    '\u{0736}',
    '\u{073A}',
    '\u{073D}',
    '\u{073F}',
    '\u{0740}',
    '\u{0741}',
    '\u{0743}',
    '\u{0745}',
    '\u{0747}',
    '\u{0749}',
    '\u{074A}',
    '\u{07EB}',
    '\u{07EC}',
    '\u{07ED}',
    '\u{07EE}',
    '\u{07EF}',
    '\u{07F0}',
    '\u{07F1}',
    '\u{07F3}',
    '\u{0816}',
    '\u{0817}',
    '\u{0818}',
    '\u{0819}',
    '\u{081B}',
    '\u{081C}',
    '\u{081D}',
    '\u{081E}',
    '\u{081F}',
    '\u{0820}',
    '\u{0821}',
    '\u{0822}',
    '\u{0823}',
    '\u{0825}',
    '\u{0826}',
    '\u{0827}',
    '\u{0829}',
    '\u{082A}',
    '\u{082B}',
    '\u{082C}',
    '\u{082D}',
    '\u{0951}',
    '\u{0953}',
    '\u{0954}',
    '\u{0F82}',
    '\u{0F83}',
    '\u{0F86}',
    '\u{0F87}',
    '\u{135D}',
    '\u{135E}',
    '\u{135F}',
    '\u{17DD}',
    '\u{193A}',
    '\u{1A17}',
    '\u{1A75}',
    '\u{1A76}',
    '\u{1A77}',
    '\u{1A78}',
    '\u{1A79}',
    '\u{1A7A}',
    '\u{1A7B}',
    '\u{1A7C}',
    '\u{1B6B}',
    '\u{1B6D}',
    '\u{1B6E}',
    '\u{1B6F}',
    '\u{1B70}',
    '\u{1B71}',
    '\u{1B72}',
    '\u{1B73}',
    '\u{1CD0}',
    '\u{1CD1}',
    '\u{1CD2}',
    '\u{1CDA}',
    '\u{1CDB}',
    '\u{1CE0}',
    '\u{1DC0}',
    '\u{1DC1}',
    '\u{1DC3}',
    '\u{1DC4}',
    '\u{1DC5}',
    '\u{1DC6}',
    '\u{1DC7}',
    '\u{1DC8}',
    '\u{1DC9}',
    '\u{1DCB}',
    '\u{1DCC}',
    '\u{1DD1}',
    '\u{1DD2}',
    '\u{1DD3}',
    '\u{1DD4}',
    '\u{1DD5}',
    '\u{1DD6}',
    '\u{1DD7}',
    '\u{1DD8}',
    '\u{1DD9}',
    '\u{1DDA}',
    '\u{1DDB}',
    '\u{1DDC}',
    '\u{1DDD}',
    '\u{1DDE}',
    '\u{1DDF}',
    '\u{1DE0}',
    '\u{1DE1}',
    '\u{1DE2}',
    '\u{1DE3}',
    '\u{1DE4}',
    '\u{1DE5}',
    '\u{1DE6}',
    '\u{1DFE}',
    '\u{20D0}',
    '\u{20D1}',
    '\u{20D4}',
    '\u{20D5}',
    '\u{20D6}',
    '\u{20D7}',
    '\u{20DB}',
    '\u{20DC}',
    '\u{20E1}',
    '\u{20E7}',
    '\u{20E9}',
    '\u{20F0}',
    '\u{2CEF}',
    '\u{2CF0}',
    '\u{2CF1}',
    '\u{2DE0}',
    '\u{2DE1}',
    '\u{2DE2}',
    '\u{2DE3}',
    '\u{2DE4}',
    '\u{2DE5}',
    '\u{2DE6}',
    '\u{2DE7}',
    '\u{2DE8}',
    '\u{2DE9}',
    '\u{2DEA}',
    '\u{2DEB}',
    '\u{2DEC}',
    '\u{2DED}',
    '\u{2DEE}',
    '\u{2DEF}',
    '\u{2DF0}',
    '\u{2DF1}',
    '\u{2DF2}',
    '\u{2DF3}',
    '\u{2DF4}',
    '\u{2DF5}',
    '\u{2DF6}',
    '\u{2DF7}',
    '\u{2DF8}',
    '\u{2DF9}',
    '\u{2DFA}',
    '\u{2DFB}',
    '\u{2DFC}',
    '\u{2DFD}',
    '\u{2DFE}',
    '\u{2DFF}',
    '\u{A66F}',
    '\u{A67C}',
    '\u{A67D}',
    '\u{A6F0}',
    '\u{A6F1}',
    '\u{A8E0}',
    '\u{A8E1}',
    '\u{A8E2}',
    '\u{A8E3}',
    '\u{A8E4}',
    '\u{A8E5}',
    '\u{A8E6}',
    '\u{A8E7}',
    '\u{A8E8}',
    '\u{A8E9}',
    '\u{A8EA}',
    '\u{A8EB}',
    '\u{A8EC}',
    '\u{A8ED}',
    '\u{A8EE}',
    '\u{A8EF}',
    '\u{A8F0}',
    '\u{A8F1}',
    '\u{AAB0}',
    '\u{AAB2}',
    '\u{AAB3}',
    '\u{AAB7}',
    '\u{AAB8}',
    '\u{AABE}',
    '\u{AABF}',
    '\u{AAC1}',
    '\u{FE20}',
    '\u{FE21}',
    '\u{FE22}',
    '\u{FE23}',
    '\u{FE24}',
    '\u{FE25}',
    '\u{FE26}',
    '\u{10A0F}',
    '\u{10A38}',
    '\u{1D185}',
    '\u{1D186}',
    '\u{1D187}',
    '\u{1D188}',
    '\u{1D189}',
    '\u{1D1AA}',
    '\u{1D1AB}',
    '\u{1D1AC}',
    '\u{1D1AD}',
    '\u{1D242}',
    '\u{1D243}',
    '\u{1D244}',
];

/// The escapes transmitting `image` under `id`: zlib-compressed RGBA, base64, chunked,
/// with the terminal's replies silenced. Nothing is displayed until it is placed.
pub(crate) fn transmit(image: &Image, id: u32) -> String {
    let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    let compressed = zlib
        .write_all(image.rgba())
        .and_then(|()| zlib.finish())
        .ok();
    let (payload, keys) = match compressed {
        Some(bytes) => (bytes, ",o=z"),
        None => (image.rgba().to_vec(), ""),
    };
    let payload = base64::engine::general_purpose::STANDARD.encode(payload);
    let chunks: Vec<&str> = payload
        .as_bytes()
        .chunks(CHUNK)
        .filter_map(|chunk| std::str::from_utf8(chunk).ok())
        .collect();
    let mut out = String::with_capacity(payload.len() + chunks.len() * 16);
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 != chunks.len());
        if i == 0 {
            out.push_str(&format!(
                "\x1b_Ga=t,i={id},f=32,s={},v={}{keys},q=2,m={more};{chunk}\x1b\\",
                image.width(),
                image.height()
            ));
        } else {
            out.push_str(&format!("\x1b_Gm={more},q=2;{chunk}\x1b\\"));
        }
    }
    out
}

/// The escape giving image `id` virtual placement `placement`, `cols`×`rows` cells.
pub(crate) fn place(id: u32, placement: u32, cols: u16, rows: u16) -> String {
    format!("\x1b_Ga=p,U=1,i={id},p={placement},c={cols},r={rows},q=2\x1b\\")
}

/// A 24-bit value as a truecolor colour, which is how a placeholder names its ids.
fn id_color(value: u32) -> Color {
    let [_, r, g, b] = value.to_be_bytes();
    Color::Rgb(r, g, b)
}

/// Fill `rect` with the placeholder cells of image `id`'s placement `placement`,
/// starting at the image's cell row `first_row` (column 0 sits at `rect.x`), on
/// background `bg` — what the terminal blends the image's transparent pixels over.
///
/// Every cell names both its row and its column, rather than inheriting them from
/// the cell to its left, so a popup covering part of a row cannot shift the rest.
pub(crate) fn paint(
    buf: &mut Buffer,
    rect: Rect,
    id: u32,
    placement: u32,
    first_row: u16,
    bg: Color,
) {
    for dy in 0..rect.height {
        let Some(&row) = DIACRITICS.get(usize::from(first_row.saturating_add(dy))) else {
            return;
        };
        for dx in 0..rect.width {
            let Some(&column) = DIACRITICS.get(usize::from(dx)) else {
                break;
            };
            let Some(cell) = buf.cell_mut((rect.x.saturating_add(dx), rect.y.saturating_add(dy)))
            else {
                continue;
            };
            cell.reset();
            cell.set_symbol(&String::from_iter([PLACEHOLDER, row, column]));
            cell.set_fg(id_color(id));
            cell.set_bg(bg);
            cell.underline_color = id_color(placement);
            if let Some(width) = NonZeroU16::new(1) {
                cell.set_diff_option(CellDiffOption::ForcedWidth(width));
            }
        }
    }
}

#[cfg(test)]
mod tests;
