use std::io::{self, Write};

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use flate2::{write::ZlibEncoder, Compression};

use crate::render::Geometry;

const CHUNK: usize = 4096;

pub struct Encoded {
    payload: String,
    pub bytes: usize,
    pub format: u8,
}

pub fn encode_pixels(rgb: &[u8], transparent_background: bool) -> Result<Encoded> {
    // Kitty f=24 is faster/lighter for an opaque RGB canvas; f=32 retains
    // terminal transparency. Multi-layer alpha blending happens before encoding.
    let mut writer = ZlibEncoder::new(Vec::new(), Compression::fast());
    writer.write_all(rgb)?;
    let compressed = writer.finish()?;
    let bytes = compressed.len();
    Ok(Encoded {
        payload: STANDARD.encode(compressed),
        bytes,
        format: if transparent_background { 32 } else { 24 },
    })
}

/// Send a Kitty graphics command in 4096-byte base64 chunks. q=1 suppresses
/// successful acknowledgements (otherwise their ESC sequences can become key events).
pub fn send<W: Write>(
    out: &mut W,
    image_id: u32,
    frame: &Encoded,
    geom: Geometry,
    y_offset: usize,
) -> io::Result<()> {
    let mut chunks = frame.payload.as_bytes().chunks(CHUNK).peekable();
    if let Some(first) = chunks.next() {
        let more = u8::from(chunks.peek().is_some());
        write!(
            out,
            "\x1b_Ga=T,f={},t=d,o=z,s={},v={},i={image_id},c={},r={},C=1,q=1,",
            frame.format, geom.width, geom.height, geom.cols, geom.rows
        )?;
        if y_offset > 0 {
            write!(out, "Y={y_offset},")?;
        }
        write!(out, "m={more};")?;
        out.write_all(first)?;
        out.write_all(b"\x1b\\")?;
        while let Some(chunk) = chunks.next() {
            let more = u8::from(chunks.peek().is_some());
            write!(out, "\x1b_Gm={more},q=1;")?;
            out.write_all(chunk)?;
            out.write_all(b"\x1b\\")?;
        }
    }
    Ok(())
}

/// Place the replacement before deleting the previous image. Kitty image IDs
/// must be distinct; reusing a live ID implicitly removes its placements.
/// Callers should allocate monotonically increasing IDs so the new frame is
/// above the old frame if both briefly occupy the same screen location.
pub fn replace<W: Write>(
    out: &mut W,
    image_id: u32,
    frame: &Encoded,
    geom: Geometry,
    y_offset: usize,
    previous_id: Option<u32>,
) -> io::Result<()> {
    send(out, image_id, frame, geom, y_offset)?;
    if let Some(previous) = previous_id {
        delete(out, previous)?;
    }
    Ok(())
}

/// Uppercase I frees this application's placement and image storage; unlike
/// a=d with no id, this does not delete other applications' images.
pub fn delete<W: Write>(out: &mut W, image_id: u32) -> io::Result<()> {
    write!(out, "\x1b_Ga=d,d=I,i={image_id},q=1\x1b\\")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacement_places_new_before_deleting_old() {
        let frame = encode_pixels(&[0u8; 3], false).unwrap();
        let geom = Geometry {
            cols: 1,
            rows: 1,
            width: 1,
            height: 1,
        };
        let mut output = Vec::new();
        replace(&mut output, 51, &frame, geom, 0, Some(50)).unwrap();
        let sequence = String::from_utf8(output).unwrap();
        let new_at = sequence.find("i=51").unwrap();
        let old_at = sequence.find("a=d,d=I,i=50").unwrap();
        assert!(new_at < old_at);
    }
    #[test]
    fn supports_subcell_vertical_placement() {
        let frame = encode_pixels(&[0; 3], false).unwrap();
        let geom = Geometry {
            cols: 1,
            rows: 1,
            width: 1,
            height: 1,
        };
        let mut output = Vec::new();
        send(&mut output, 7, &frame, geom, 6).unwrap();
        assert!(String::from_utf8(output).unwrap().contains(",Y=6,m="));
    }
    #[test]
    fn rgb_and_rgba_have_distinct_format_headers() {
        for (alpha, bytes, format) in [(false, 3, "f=24"), (true, 4, "f=32")] {
            let frame = encode_pixels(&vec![0u8; bytes], alpha).unwrap();
            let geom = Geometry {
                cols: 1,
                rows: 1,
                width: 1,
                height: 1,
            };
            let mut output = Vec::new();
            send(&mut output, 100, &frame, geom, 0).unwrap();
            let txt = String::from_utf8(output).unwrap();
            assert!(txt.contains(format));
        }
    }
    #[test]
    fn encodes_valid_chunked_kitty_sequence() {
        // Incompressible enough to require many 4096-byte chunks.
        let pixels: Vec<u8> = (0..300u64 * 400 * 4)
            .map(|i| ((i.wrapping_mul(1_103_515_245).wrapping_add(12_345) >> 12) & 255) as u8)
            .collect();
        let frame = encode_pixels(&pixels, true).unwrap();
        let geom = Geometry {
            cols: 20,
            rows: 10,
            width: 400,
            height: 300,
        };
        let mut result = Vec::new();
        send(&mut result, 42, &frame, geom, 0).unwrap();
        let text = String::from_utf8(result).unwrap();
        assert!(text.starts_with("\x1b_Ga=T,f=32,t=d,o=z,s=400,v=300,i=42,c=20,r=10,C=1,q=1,m="));
        assert!(text.ends_with("\x1b\\"));
        assert!(text.contains("\x1b_Gm=1,q=1;"));
        assert!(text.contains("\x1b_Gm=0,q=1;"));
        assert!(!text.contains("q=2"));
        let mut deletion = Vec::new();
        delete(&mut deletion, 42).unwrap();
        assert_eq!(deletion, b"\x1b_Ga=d,d=I,i=42,q=1\x1b\\");
    }
}
