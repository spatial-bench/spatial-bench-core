//! Terminal interaction for the graphics protocol: isatty, cell-size query,
//! and the raw-mode response reader the probe needs.

use std::io::Write;

/// True when stdin is a terminal — the gate for interactive prompts and for
/// attempting the graphics handshake at all.
pub fn stdin_is_tty() -> bool {
    unsafe { libc::isatty(0) == 1 }
}

/// The terminal's cell size in pixels: `(width, height)`, queried with the
/// XTWINOPS cell report (`CSI 16 t`) and a raw-mode read with a short
/// timeout. Falls back to a conventional 10×20 cell when the terminal does
/// not answer (piped output, odd emulators).
pub fn cell_size() -> (u32, u32) {
    let Some(response) = read_csi_responses("\x1b[16t", 250) else {
        return (10, 20);
    };
    // The response is `CSI 6 ; <height> ; <width> t`.
    if let Some(rest) = response.strip_prefix("\x1b[6;") {
        let nums: Vec<u32> = rest
            .trim_end_matches('t')
            .split(';')
            .filter_map(|n| n.parse().ok())
            .collect();
        if let [height, width] = nums[..] {
            if width > 0 && height > 0 {
                return (width, height);
            }
        }
    }
    (10, 20)
}

/// Write `query`, then read the terminal's response(s) in raw mode with a
/// timeout, returning everything that arrived. Restores the terminal state
/// no matter what.
pub fn read_csi_responses(query: &str, timeout_ms: u64) -> Option<String> {
    let fd = 0;
    let mut saved = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
        return None;
    }
    let mut raw = saved;
    unsafe { libc::cfmakeraw(&mut raw) };
    // Non-blocking reads: return whatever is available after VTIME tenths of
    // a second, so the timeout is enforced by the read loop's budget.
    raw.c_cc[libc::VMIN] = 0;
    raw.c_cc[libc::VTIME] = 2;
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
        return None;
    }

    let result = (|| {
        let mut out = std::io::stdout();
        out.write_all(query.as_bytes()).ok()?;
        out.flush().ok()?;
        let mut response = String::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        let mut buf = [0u8; 1024];
        loop {
            if std::time::Instant::now() >= deadline {
                break;
            }
            let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                continue;
            }
            response.push_str(&String::from_utf8_lossy(&buf[..n as usize]));
            // The DA1 response ("...c") is the last thing the terminal sends
            // after the graphics reply, so it terminates the read.
            if response.ends_with('c') {
                break;
            }
        }
        if response.is_empty() {
            None
        } else {
            Some(response)
        }
    })();

    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &saved) };
    result
}
