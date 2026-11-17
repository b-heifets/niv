use std::io::{self, Write};

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use flate2::{write::ZlibEncoder, Compression};

use crate::render::Geometry;

const CHUNK: usize = 4096;

pub struct Encoded {
    payload: String,
    pub bytes: usize,
}

pub fn encode_rgba(rgb: &[u8]) -> Result<Encoded> {
    // Kitty RGBA32 + zlib preserves truly transparent pixels across layers.
    let mut writer = ZlibEncoder::new(Vec::new(), Compression::fast());
    writer.write_all(rgb)?;
    let compressed = writer.finish()?;
    let bytes = compressed.len();
    Ok(Encoded {
        payload: STANDARD.encode(compressed),
        bytes,
    })
}

/// Send a Kitty graphics command in 4096-byte base64 chunks. q=1 suppresses
/// successful acknowledgements (otherwise their ESC sequences can become key events).
pub fn send<W: Write>(
    out: &mut W,
    image_id: u32,
    frame: &Encoded,
    geom: Geometry,
) -> io::Result<()> {
    let mut chunks = frame.payload.as_bytes().chunks(CHUNK).peekable();
    if let Some(first) = chunks.next() {
        let more = u8::from(chunks.peek().is_some());
        write!(
            out,
            "\x1b_Ga=T,f=32,t=d,o=z,s={},v={},i={image_id},c={},r={},C=1,q=1,m={more};",
            geom.width, geom.height, geom.cols, geom.rows
        )?;
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
    previous_id: Option<u32>,
) -> io::Result<()> {
    send(out, image_id, frame, geom)?;
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
        let frame = encode_rgba(&[0u8; 4]).unwrap();
        let geom = Geometry {
            cols: 1,
            rows: 1,
            width: 1,
            height: 1,
        };
        let mut output = Vec::new();
        replace(&mut output, 51, &frame, geom, Some(50)).unwrap();
        let sequence = String::from_utf8(output).unwrap();
        let new_at = sequence.find("i=51").unwrap();
        let old_at = sequence.find("a=d,d=I,i=50").unwrap();
        assert!(new_at < old_at);
    }
    #[test]
    fn encodes_valid_chunked_kitty_sequence() {
        // Incompressible enough to require many 4096-byte chunks.
        let pixels: Vec<u8> = (0..300u64 * 400 * 4)
            .map(|i| ((i.wrapping_mul(1_103_515_245).wrapping_add(12_345) >> 12) & 255) as u8)
            .collect();
        let frame = encode_rgba(&pixels).unwrap();
        let geom = Geometry {
            cols: 20,
            rows: 10,
            width: 400,
            height: 300,
        };
        let mut result = Vec::new();
        send(&mut result, 42, &frame, geom).unwrap();
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
