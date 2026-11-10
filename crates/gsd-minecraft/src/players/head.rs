//! A player's head, cut from their skin: the face (8×8 at 8,8) with the hat layer (at 40,8) over it,
//! where the game would draw it, scaled up without smoothing so it stays crisp.

use std::io::Cursor;

use png::{BitDepth, ColorType, Decoder, Encoder, Limits, Transformations};

/// The face is 8 pixels square; heads are served at this many times that.
const SCALE: usize = 8;
pub const HEAD_SIZE: usize = 8 * SCALE;
/// Skins are 64×64 (or 64×32 from before 1.8). Anything needing more memory than this isn't one.
const DECODE_LIMIT_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum HeadError {
    #[error("the skin isn't a PNG this can read: {0}")]
    Decode(#[from] png::DecodingError),
    #[error("the skin is {0}×{1}, not a Minecraft skin")]
    Size(u32, u32),
    #[error("could not write the head: {0}")]
    Encode(#[from] png::EncodingError),
}

/// The head as a PNG of [`HEAD_SIZE`] square.
pub fn render(skin: &[u8]) -> Result<Vec<u8>, HeadError> {
    let (height, pixels) = decode_rgba(skin)?;
    let pixel = |x: usize, y: usize| -> [u8; 4] {
        let at = (y * 64 + x) * 4;
        [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
    };
    let hat = hat_shows(height, &pixels);
    let mut head = vec![0_u8; HEAD_SIZE * HEAD_SIZE * 4];
    for y in 0..8 {
        for x in 0..8 {
            let face = pixel(8 + x, 8 + y);
            let color = if hat {
                over(pixel(40 + x, 8 + y), face)
            } else {
                over([0; 4], face)
            };
            for dy in 0..SCALE {
                let row = (y * SCALE + dy) * HEAD_SIZE;
                for dx in 0..SCALE {
                    let at = (row + x * SCALE + dx) * 4;
                    head[at..at + 4].copy_from_slice(&color);
                }
            }
        }
    }
    encode(&head)
}

/// The skin's height (64 or 32) and its pixels, four bytes each, 64 to a row.
fn decode_rgba(skin: &[u8]) -> Result<(usize, Vec<u8>), HeadError> {
    let mut decoder = Decoder::new_with_limits(
        Cursor::new(skin),
        Limits {
            bytes: DECODE_LIMIT_BYTES,
        },
    );
    decoder.set_transformations(Transformations::normalize_to_color8() | Transformations::ALPHA);
    let mut reader = decoder.read_info()?;
    let (width, height) = (reader.info().width, reader.info().height);
    if width != 64 || !(height == 64 || height == 32) {
        return Err(HeadError::Size(width, height));
    }
    let mut buffer = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or(HeadError::Size(width, height))?
    ];
    let info = reader.next_frame(&mut buffer)?;
    buffer.truncate(info.buffer_size());
    let rgba = match info.color_type {
        ColorType::Rgba => buffer,
        ColorType::Rgb => buffer
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, 255])
            .collect(),
        ColorType::GrayscaleAlpha => buffer
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[g, a]| [g, g, g, a])
            .collect(),
        ColorType::Grayscale => buffer.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        // Expanded away by the transformations above.
        ColorType::Indexed => return Err(HeadError::Size(width, height)),
    };
    Ok((usize::try_from(height).unwrap_or(64), rgba))
}

/// Whether the hat layer shows. A skin from before 1.8 (64×32) had no transparency, so many have the hat
/// area painted solid; the game ignores the hat of such a skin when nothing in that area is see-through
/// (its "Notch transparency hack", named after the skin it was made for). Newer skins always show theirs.
fn hat_shows(height: usize, pixels: &[u8]) -> bool {
    if height == 64 {
        return true;
    }
    (0..32).any(|y| (32..64).any(|x| pixels[(y * 64 + x) * 4 + 3] < 128))
}

/// `top` over `bottom`. The face itself is opaque in the game, so it is here too.
fn over(top: [u8; 4], bottom: [u8; 4]) -> [u8; 4] {
    let alpha = u16::from(top[3]);
    let mix = |t: u8, b: u8| {
        let mixed = (u16::from(t) * alpha + u16::from(b) * (255 - alpha)) / 255;
        u8::try_from(mixed).unwrap_or(u8::MAX)
    };
    [
        mix(top[0], bottom[0]),
        mix(top[1], bottom[1]),
        mix(top[2], bottom[2]),
        255,
    ]
}

fn encode(rgba: &[u8]) -> Result<Vec<u8>, HeadError> {
    let mut out = Vec::new();
    let side = u32::try_from(HEAD_SIZE).unwrap_or(64);
    let mut encoder = Encoder::new(&mut out, side, side);
    encoder.set_color(ColorType::Rgba);
    encoder.set_depth(BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    writer.finish()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::test_support::test_skin;

    fn pixels_of(png_bytes: &[u8]) -> (u32, Vec<u8>) {
        let decoder = Decoder::new(Cursor::new(png_bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buffer).unwrap();
        (info.width, buffer)
    }

    #[test]
    fn the_face_with_its_hat_is_cut_out_and_scaled_up() {
        for height in [64, 32] {
            let head = render(&test_skin(height)).unwrap();
            let (width, pixels) = pixels_of(&head);
            assert_eq!(width as usize, HEAD_SIZE);
            let at = |x: usize, y: usize| &pixels[(y * HEAD_SIZE + x) * 4..(y * HEAD_SIZE + x) * 4 + 4];
            assert_eq!(at(0, 0), [0, 0, 255, 255], "the hat's pixel covers the face");
            assert_eq!(at(7, 7), [0, 0, 255, 255], "one skin pixel is 8×8");
            assert_eq!(at(8, 0), [200, 0, 0, 255], "the face shows through a clear hat");
            assert_eq!(at(63, 63), [200, 0, 0, 255]);
        }
    }

    /// No transparency anywhere, as skins from before 1.8: a red face, the hat area solid black, the
    /// rest grey.
    fn skin_with_solid_hat(height: u32) -> Vec<u8> {
        let mut pixels = vec![0_u8; 64 * height as usize * 4];
        for y in 0..height as usize {
            for x in 0..64 {
                let at = (y * 64 + x) * 4;
                let color = if (8..16).contains(&x) && (8..16).contains(&y) {
                    [200, 0, 0, 255]
                } else if x >= 32 && y < 16 {
                    [0, 0, 0, 255]
                } else {
                    [90, 90, 90, 255]
                };
                pixels[at..at + 4].copy_from_slice(&color);
            }
        }
        let mut out = Vec::new();
        let mut encoder = Encoder::new(&mut out, 64, height);
        encoder.set_color(ColorType::Rgba);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&pixels).unwrap();
        writer.finish().unwrap();
        out
    }

    /// Regression (Notch's real skin, a 64×32 one): its hat area is solid black, which the game doesn't
    /// draw, and the head came out black.
    #[test]
    fn an_old_skins_solid_hat_is_left_off_as_the_game_does() {
        let (_, pixels) = pixels_of(&render(&skin_with_solid_hat(32)).unwrap());
        assert_eq!(&pixels[..4], [200, 0, 0, 255], "the face, not the hat");
        let (_, pixels) = pixels_of(&render(&skin_with_solid_hat(64)).unwrap());
        assert_eq!(&pixels[..4], [0, 0, 0, 255], "a new skin's hat always shows");
    }

    #[test]
    fn what_isnt_a_skin_is_refused() {
        assert!(matches!(render(b"not a png"), Err(HeadError::Decode(_))));
        let mut big = Vec::new();
        let mut encoder = Encoder::new(&mut big, 16, 16);
        encoder.set_color(ColorType::Rgba);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[0; 16 * 16 * 4]).unwrap();
        writer.finish().unwrap();
        assert!(matches!(render(&big), Err(HeadError::Size(16, 16))));
    }
}
