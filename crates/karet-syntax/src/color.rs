//! Color-literal detection for inline swatches: hex (`#rgb`, `#rgba`,
//! `#rrggbb`, `#rrggbbaa`), `rgb()`/`rgba()`, and `hsl()`/`hsla()` forms.
//!
//! Pure text-in/data-out over one line at a time — the caller (an editor)
//! scans its visible lines each frame and turns the answers into swatch
//! decorations. Ranges are **character** columns, matching caret coordinates.

use std::ops::Range;

/// Every color literal on `line`, as `(character range, straight RGBA)`.
#[must_use]
pub fn detect(line: &str) -> Vec<(Range<usize>, [u8; 4])> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '#' => {
                if let Some((len, rgba)) = hex_at(&chars, i) {
                    out.push((i..i + len, rgba));
                    i += len;
                    continue;
                }
            },
            'r' | 'R' | 'h' | 'H' => {
                if let Some((len, rgba)) = function_at(&chars, i) {
                    out.push((i..i + len, rgba));
                    i += len;
                    continue;
                }
            },
            _ => {},
        }
        i += 1;
    }
    out
}

/// A `#hex` literal starting at `at`, when one is there with word boundaries.
fn hex_at(chars: &[char], at: usize) -> Option<(usize, [u8; 4])> {
    // `#` preceded by a word character is an anchor/id, not a color.
    if at > 0 && (chars[at - 1].is_alphanumeric() || chars[at - 1] == '_' || chars[at - 1] == '&') {
        return None;
    }
    let digits: Vec<char> = chars[at + 1..]
        .iter()
        .take_while(|c| c.is_ascii_hexdigit())
        .copied()
        .collect();
    // A longer hex run (a commit hash, a hash fragment) is not a color.
    if chars
        .get(at + 1 + digits.len())
        .is_some_and(|c| c.is_alphanumeric() || *c == '_')
    {
        return None;
    }
    let nib = |c: char| c.to_digit(16).map(|d| d as u8);
    let rgba = match digits.len() {
        3 | 4 => {
            let mut v = [0u8; 4];
            v[3] = 0xff;
            for (slot, c) in v.iter_mut().zip(&digits) {
                let d = nib(*c)?;
                *slot = d << 4 | d;
            }
            v
        },
        6 | 8 => {
            let mut v = [0u8; 4];
            v[3] = 0xff;
            for (index, pair) in digits.chunks(2).enumerate().take(4) {
                v[index] = nib(pair[0])? << 4 | nib(pair[1])?;
            }
            v
        },
        _ => return None,
    };
    Some((1 + digits.len(), rgba))
}

/// An `rgb()`/`rgba()`/`hsl()`/`hsla()` literal starting at `at`.
fn function_at(chars: &[char], at: usize) -> Option<(usize, [u8; 4])> {
    if at > 0 && (chars[at - 1].is_alphanumeric() || chars[at - 1] == '_' || chars[at - 1] == '-') {
        return None;
    }
    let rest: String = chars[at..].iter().take(64).collect();
    let lower = rest.to_ascii_lowercase();
    let (name_len, hsl) = if lower.starts_with("rgba(") {
        (5, false)
    } else if lower.starts_with("rgb(") {
        (4, false)
    } else if lower.starts_with("hsla(") {
        (5, true)
    } else if lower.starts_with("hsl(") {
        (4, true)
    } else {
        return None;
    };
    let close = rest.find(')')?;
    let inner = &rest[name_len..close];
    let parts: Vec<&str> = inner
        .split(|c: char| c == ',' || c.is_whitespace() || c == '/')
        .filter(|p| !p.is_empty())
        .collect();
    if !(3..=4).contains(&parts.len()) {
        return None;
    }
    let alpha = match parts.get(3) {
        None => 0xff,
        Some(a) => {
            let a = a.strip_suffix('%').map_or_else(
                || a.parse::<f32>().ok(),
                |pct| pct.parse::<f32>().ok().map(|v| v / 100.0),
            )?;
            (a.clamp(0.0, 1.0) * 255.0).round() as u8
        },
    };
    let channel = |part: &str, scale: f32| -> Option<f32> {
        part.strip_suffix('%').map_or_else(
            || part.parse::<f32>().ok(),
            |pct| pct.parse::<f32>().ok().map(|v| v / 100.0 * scale),
        )
    };
    let rgba = if hsl {
        let hue = parts[0]
            .trim_end_matches("deg")
            .parse::<f32>()
            .ok()?
            .rem_euclid(360.0);
        let saturation = channel(parts[1], 1.0)
            .map(|v| if v > 1.0 { v / 100.0 } else { v })?
            .clamp(0.0, 1.0);
        let lightness = channel(parts[2], 1.0)
            .map(|v| if v > 1.0 { v / 100.0 } else { v })?
            .clamp(0.0, 1.0);
        let [r, g, b] = hsl_to_rgb(hue, saturation, lightness);
        [r, g, b, alpha]
    } else {
        let mut v = [0u8; 4];
        v[3] = alpha;
        for (slot, part) in v.iter_mut().zip(&parts) {
            let value = channel(part, 255.0)?;
            *slot = value.clamp(0.0, 255.0).round() as u8;
        }
        v[3] = alpha;
        v
    };
    Some((at_len(name_len, close), rgba))
}

/// The literal's full character length (`name(` through `)`).
const fn at_len(_name_len: usize, close: usize) -> usize {
    close + 1
}

/// Standard HSL → sRGB conversion.
fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> [u8; 3] {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let hue_prime = hue / 60.0;
    let x = chroma * (1.0 - (hue_prime.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hue_prime as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = lightness - chroma / 2.0;
    [
        ((r1 + m).clamp(0.0, 1.0) * 255.0).round() as u8,
        ((g1 + m).clamp(0.0, 1.0) * 255.0).round() as u8,
        ((b1 + m).clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_forms_parse_with_boundaries() {
        assert_eq!(detect("color: #fff;"), vec![(7..11, [255, 255, 255, 255])]);
        assert_eq!(detect("#1a2b3c"), vec![(0..7, [0x1a, 0x2b, 0x3c, 255])]);
        assert_eq!(detect("#12345678"), vec![(0..9, [0x12, 0x34, 0x56, 0x78])]);
        assert_eq!(detect("#abcd"), vec![(0..5, [0xaa, 0xbb, 0xcc, 0xdd])]);
        // Wrong lengths and word-adjacent hashes are not colors.
        assert!(detect("#12345 and #12").is_empty());
        assert!(detect("a#fff").is_empty());
        assert!(detect("commit #1a2b3c4d5e").is_empty());
    }

    #[test]
    fn rgb_functions_parse_all_syntaxes() {
        assert_eq!(
            detect("rgb(255, 128, 0)"),
            vec![(0..16, [255, 128, 0, 255])]
        );
        assert_eq!(detect("rgba(0, 0, 0, 0.5)"), vec![(0..18, [0, 0, 0, 128])]);
        // Modern space/slash syntax and percentages.
        assert_eq!(
            detect("rgb(100% 0% 0% / 50%)"),
            vec![(0..21, [255, 0, 0, 128])]
        );
        assert!(detect("rgb(1, 2)").is_empty());
        assert!(detect("word_rgb(1, 2, 3)").is_empty());
    }

    #[test]
    fn hsl_functions_convert() {
        assert_eq!(detect("hsl(0, 100%, 50%)"), vec![(0..17, [255, 0, 0, 255])]);
        assert_eq!(
            detect("hsl(120, 100%, 25%)"),
            vec![(0..19, [0, 128, 0, 255])]
        );
        assert_eq!(
            detect("hsla(240, 100%, 50%, 0.5)"),
            vec![(0..25, [0, 0, 255, 128])]
        );
    }

    #[test]
    fn several_literals_on_one_line_all_report() {
        let found = detect("border: 1px solid #ff0000; background: rgb(0, 255, 0);");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].1, [255, 0, 0, 255]);
        assert_eq!(found[1].1, [0, 255, 0, 255]);
    }

    #[test]
    fn multibyte_prefixes_keep_character_columns() {
        let found = detect("héllo #00ff00");
        assert_eq!(found, vec![(6..13, [0, 255, 0, 255])]);
    }
}
