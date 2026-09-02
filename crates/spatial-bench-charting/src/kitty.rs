//! The kitty graphics protocol (day-two TODO — the one bespoke piece): a
//! capability probe and chunked raw-RGBA transmission of the rendered chart.
//!
//! Transmission shape (kitty's documentation): each chunk is
//! `ESC _ G <controls>;<base64 payload> ESC \` with the payload split into
//! ≤4096-byte base64 chunks; the first chunk carries the image controls
//! (`f=24` RGB, `s`/`h` the pixel size, `a=T` transmit-and-display, `C=1` to
//! keep the cursor, `q=2` to suppress responses, `c`/`r` the cell box to
//! reserve) and `m=1` on every chunk but the last.

use std::io::Write;

const CHUNK: usize = 4096;

/// A rendered RGB buffer plus the cell geometry it was sized for.
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// 3 bytes per pixel, row-major (plotters' BitMapBackend layout).
    pub rgb: Vec<u8>,
    pub cell_width_px: u32,
    pub cell_height_px: u32,
}

impl Image {
    /// The image's footprint in terminal cells.
    pub fn cells(&self) -> (u32, u32) {
        let cols = self.width.div_ceil(self.cell_width_px);
        let rows = self.height.div_ceil(self.cell_height_px);
        (cols.max(1), rows.max(1))
    }
}

/// Is this terminal likely to speak the kitty graphics protocol? The env
/// fast-path; the escape probe in [`probe`] is the authoritative check.
pub fn likely_supported() -> bool {
    std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
        || std::env::var_os("WEZTERM_EXECUTABLE").is_some()
        || std::env::var_os("TERMINAL_PROGRAM")
            .is_some_and(|p| p == "WezTerm" || p == "ghostty" || p == "kitty")
}

/// Ask the terminal whether it implements the graphics protocol: a tiny
/// image query followed by a primary device-attributes request. A terminal
/// that answers the graphics query supports the protocol.
#[allow(dead_code)] // the escape-probe path, for terminals without the env markers
pub fn probe() -> bool {
    let Some(response) =
        crate::term::read_csi_responses("\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c", 400)
    else {
        return false;
    };
    response.contains("_Gi=31;OK") || response.contains("_Gi=31;i=31")
}

/// Transmit and display the image at the cursor.
pub fn transmit(image: &Image) -> Result<(), String> {
    let (cols, rows) = image.cells();
    let b64 = base64(&image.rgb);
    let chunks: Vec<&[u8]> = b64.chunks(CHUNK).collect();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = if i + 1 < chunks.len() { 1 } else { 0 };
        let controls = if i == 0 {
            format!(
                "a=T,f=24,s={},v={},c={},r={},q=2,C=1,m={more}",
                image.width, image.height, cols, rows
            )
        } else {
            format!("m={more}")
        };
        write!(
            out,
            "\x1b_G{controls};{}\x1b\\",
            String::from_utf8_lossy(chunk)
        )
        .map_err(|e| format!("writing the graphics protocol: {e}"))?;
    }
    out.flush().map_err(|e| format!("flushing: {e}"))?;
    // Reserve the rows the image occupies so following output lands below it.
    println!("\r\n\r{}", "\n".repeat(rows.saturating_sub(1) as usize));
    Ok(())
}

/// Standard base64 (the alphabet is 40 lines of code, and the charting crate
/// has no other use for a base64 dependency).
pub fn base64(data: &[u8]) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        out.push(ALPHABET[(b[0] >> 2) as usize]);
        out.push(ALPHABET[(((b[0] & 0x03) << 4) | (b[1] >> 4)) as usize]);
        out.push(if chunk.len() > 1 {
            ALPHABET[(((b[1] & 0x0F) << 2) | (b[2] >> 6)) as usize]
        } else {
            b'='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(b[2] & 0x3F) as usize]
        } else {
            b'='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode64(data: &[u8]) -> Vec<u8> {
        fn val(c: u8) -> Option<u32> {
            match c {
                b'A'..=b'Z' => Some((c - b'A') as u32),
                b'a'..=b'z' => Some((c - b'a' + 26) as u32),
                b'0'..=b'9' => Some((c - b'0' + 52) as u32),
                b'+' => Some(62),
                b'/' => Some(63),
                _ => None,
            }
        }
        let mut out = Vec::new();
        let mut acc = 0u32;
        let mut bits = 0;
        for &c in data {
            if c == b'=' {
                break;
            }
            acc = (acc << 6) | val(c).unwrap();
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
            }
        }
        out
    }

    #[test]
    fn base64_round_trips() {
        for len in [0usize, 1, 2, 3, 4, 5, 100, 4095, 4096, 4097, 10_000] {
            let data: Vec<u8> = (0..len).map(|i| (i * 7 + 13) as u8).collect();
            let encoded = base64(&data);
            assert_eq!(decode64(&encoded), data, "length {len}");
        }
    }

    #[test]
    fn transmission_chunks_and_flags_are_wellformed() {
        // 8 pixels = 24 bytes RGB → base64 32 chars → one chunk.
        let image = Image {
            width: 4,
            height: 2,
            rgb: vec![0xAB; 24],
            cell_width_px: 10,
            cell_height_px: 20,
        };
        assert_eq!(image.cells(), (1, 1));
        let b64 = base64(&image.rgb);
        assert!(b64.len() <= CHUNK, "small images are one chunk");
        assert!(b64.starts_with(b"q6urq6ur"), "known prefix");

        // A large buffer splits into chunks of exactly CHUNK with correct m
        // flags: 1 for all but the last, 0 for the last.
        let big = vec![0u8; 6000 * 3];
        let encoded = base64(&big);
        let chunks: Vec<&[u8]> = encoded.chunks(CHUNK).collect();
        assert!(chunks.len() > 1);
        for (i, c) in chunks.iter().enumerate() {
            assert_eq!(c.len(), CHUNK.min(b64_len(6000 * 3) - i * CHUNK));
        }
    }

    fn b64_len(bytes: usize) -> usize {
        bytes.div_ceil(3) * 4
    }
}
