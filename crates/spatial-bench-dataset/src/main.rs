//! The dataset generator: streams binary points to stdout for a driver to
//! consume. Replaces the per-language ChaCha8 generators (day 1.5 phase 2 of
//! the dataset work) — one Rust implementation, every language reads its
//! output.
//!
//! Binary format (native endianness, never leaves this machine):
//! ```text
//! header: "SBDS" + u32 version(1) + u32 dims + u8 dtype + u64 tree_count
//!              + u64 query_count
//! body:   tree_count × dims × sizeof(dtype) bytes   (construction points)
//!         query_count × dims × sizeof(dtype) bytes   (query points)
//! ```
//!
//! The stream is ChaCha8 (matching the previous per-language generators
//! byte-for-byte for the `uniform` kind), seeded from the `--seed` parameter.

use std::io::Write;

const MAGIC: &[u8; 4] = b"SBDS";
const VERSION: u32 = 1;

#[derive(Clone, Copy, PartialEq)]
enum DType {
    F32,
    F64,
}

impl DType {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "f32" => Some(DType::F32),
            "f64" => Some(DType::F64),
            _ => None,
        }
    }
    fn as_u8(&self) -> u8 {
        match self {
            DType::F32 => 0,
            DType::F64 => 1,
        }
    }
}

/// ChaCha8 stream matching rand_chacha's ChaCha8Rng (rand 0.10).
/// 64-byte blocks, 8 rounds, counter from zero, u32 words in order.
struct ChaCha8 {
    key: [u32; 8],
    counter: u64,
    buf: [u32; 16],
    index: usize,
}

impl ChaCha8 {
    fn new(seed: u64) -> Self {
        // rand_core's `seed_from_u64`: PCG32 expansion, 4 bytes at a time.
        const MUL: u64 = 6364136223846793005;
        const INC: u64 = 11634580027462260723;
        let mut state = seed;
        let mut seed_bytes = [0u8; 32];
        for chunk in 0..8 {
            state = state.wrapping_mul(MUL).wrapping_add(INC);
            let xorshifted = (((state >> 18) ^ state) >> 27) as u32;
            let rot = (state >> 59) as u32;
            let x = xorshifted.rotate_right(rot);
            seed_bytes[chunk * 4..chunk * 4 + 4].copy_from_slice(&x.to_le_bytes());
        }
        let mut key = [0u32; 8];
        for (i, k) in key.iter_mut().enumerate() {
            *k = u32::from_le_bytes(seed_bytes[i * 4..i * 4 + 4].try_into().unwrap());
        }
        ChaCha8 {
            key,
            counter: 0,
            buf: [0; 16],
            index: 16,
        }
    }

    fn quarter(w: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
        w[a] = w[a].wrapping_add(w[b]);
        w[d] ^= w[a];
        w[d] = w[d].rotate_left(16);
        w[c] = w[c].wrapping_add(w[d]);
        w[b] ^= w[c];
        w[b] = w[b].rotate_left(12);
        w[a] = w[a].wrapping_add(w[b]);
        w[d] ^= w[a];
        w[d] = w[d].rotate_left(8);
        w[c] = w[c].wrapping_add(w[d]);
        w[b] ^= w[c];
        w[b] = w[b].rotate_left(7);
    }

    fn refill(&mut self) {
        let mut st = [0u32; 16];
        st[0] = 0x61707865;
        st[1] = 0x3320646e;
        st[2] = 0x79622d32;
        st[3] = 0x6b206574;
        st[4..12].copy_from_slice(&self.key);
        st[12] = self.counter as u32;
        st[13] = (self.counter >> 32) as u32;
        st[14] = 0;
        st[15] = 0;
        let mut w = st;
        for _ in 0..4 {
            Self::quarter(&mut w, 0, 4, 8, 12);
            Self::quarter(&mut w, 1, 5, 9, 13);
            Self::quarter(&mut w, 2, 6, 10, 14);
            Self::quarter(&mut w, 3, 7, 11, 15);
            Self::quarter(&mut w, 0, 5, 10, 15);
            Self::quarter(&mut w, 1, 6, 11, 12);
            Self::quarter(&mut w, 2, 7, 8, 13);
            Self::quarter(&mut w, 3, 4, 9, 14);
        }
        for i in 0..16 {
            self.buf[i] = w[i].wrapping_add(st[i]);
        }
        self.counter += 1;
        self.index = 0;
    }

    fn next_u32(&mut self) -> u32 {
        if self.index >= 16 {
            self.refill();
        }
        let v = self.buf[self.index];
        self.index += 1;
        v
    }

    fn next_u64(&mut self) -> u64 {
        let lo = self.next_u32() as u64;
        let hi = self.next_u32() as u64;
        (hi << 32) | lo
    }

    /// rand's StandardUniform for f64.
    fn next_f64(&mut self) -> f64 {
        let x = self.next_u64();
        ((x >> 11) as f64) * (1.0 / 9007199254740992.0)
    }

    /// rand's StandardUniform for f32.
    fn next_f32(&mut self) -> f32 {
        let x = self.next_u32();
        ((x >> 8) as f32) * (1.0 / 16777216.0)
    }
}

/// Generate `count` points of `dims` dimensions from the uniform [0,1)^D
/// distribution. Matches the previous per-language generators byte-for-byte.
fn generate_uniform_f64(seed: u64, count: usize, dims: usize) -> Vec<f64> {
    let mut rng = ChaCha8::new(seed);
    let mut out = Vec::with_capacity(count * dims);
    for _ in 0..count {
        for _ in 0..dims {
            out.push(rng.next_f64());
        }
    }
    out
}

fn generate_uniform_f32(seed: u64, count: usize, dims: usize) -> Vec<f32> {
    let mut rng = ChaCha8::new(seed);
    let mut out = Vec::with_capacity(count * dims);
    for _ in 0..count {
        for _ in 0..dims {
            out.push(rng.next_f32());
        }
    }
    out
}

fn generate_gaussian_f64(seed: u64, count: usize, dims: usize) -> Vec<f64> {
    let mut rng = ChaCha8::new(seed);
    let mut out = Vec::with_capacity(count * dims);
    let total = count * dims;
    while out.len() < total {
        let u1 = rng.next_f64();
        let u2 = rng.next_f64();
        if u1 <= 0.0 {
            continue;
        }
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f64::consts::PI * u2;
        out.push(r * theta.cos());
        if out.len() < total {
            out.push(r * theta.sin());
        }
    }
    out
}

fn generate_gaussian_f32(seed: u64, count: usize, dims: usize) -> Vec<f32> {
    let mut rng = ChaCha8::new(seed);
    let mut out = Vec::with_capacity(count * dims);
    let total = count * dims;
    while out.len() < total {
        let u1 = rng.next_f32() as f64;
        let u2 = rng.next_f32() as f64;
        if u1 <= 0.0 {
            continue;
        }
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f64::consts::PI * u2;
        out.push((r * theta.cos()) as f32);
        if out.len() < total {
            out.push((r * theta.sin()) as f32);
        }
    }
    out
}

fn write_header(
    out: &mut impl Write,
    dims: usize,
    dtype: &DType,
    tree_count: u64,
    query_count: u64,
) -> Result<(), String> {
    out.write_all(MAGIC).map_err(|e| e.to_string())?;
    out.write_all(&VERSION.to_ne_bytes())
        .map_err(|e| e.to_string())?;
    out.write_all(&(dims as u32).to_ne_bytes())
        .map_err(|e| e.to_string())?;
    out.write_all(&[dtype.as_u8()]).map_err(|e| e.to_string())?;
    out.write_all(&tree_count.to_ne_bytes())
        .map_err(|e| e.to_string())?;
    out.write_all(&query_count.to_ne_bytes())
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn write_points(out: &mut impl Write, points: &[u8]) -> Result<(), String> {
    out.write_all(points).map_err(|e| e.to_string())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut kind = String::from("uniform");
    let mut dims: usize = 3;
    let mut dtype = DType::F64;
    let mut tree_count: u64 = 1000;
    let mut query_count: u64 = 100;
    let mut seed: u64 = 42;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--kind" => {
                kind = args.get(i + 1).cloned().unwrap_or_default();
                i += 2;
            }
            "--dims" => {
                dims = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(3);
                i += 2;
            }
            "--dtype" => {
                dtype = args
                    .get(i + 1)
                    .and_then(|v| DType::from_str(v))
                    .unwrap_or(DType::F64);
                i += 2;
            }
            "--tree-count" => {
                tree_count = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(1000);
                i += 2;
            }
            "--query-count" => {
                query_count = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(100);
                i += 2;
            }
            "--seed" => {
                seed = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(42);
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    let f64_mode = dtype == DType::F64;
    let tree_seed = seed;
    let query_seed = seed.wrapping_add(1);

    let gen_bytes = |s: u64, count: u64| -> Vec<u8> {
        let c = count as usize;
        match (kind.as_str(), f64_mode) {
            ("uniform", true) => generate_uniform_f64(s, c, dims)
                .iter()
                .flat_map(|v| v.to_ne_bytes())
                .collect(),
            ("uniform", false) => generate_uniform_f32(s, c, dims)
                .iter()
                .flat_map(|v| v.to_ne_bytes())
                .collect(),
            ("gaussian", true) => generate_gaussian_f64(s, c, dims)
                .iter()
                .flat_map(|v| v.to_ne_bytes())
                .collect(),
            ("gaussian", false) => generate_gaussian_f32(s, c, dims)
                .iter()
                .flat_map(|v| v.to_ne_bytes())
                .collect(),
            _ => {
                eprintln!("unknown kind");
                Vec::new()
            }
        }
    };

    let tree_bytes = gen_bytes(tree_seed, tree_count);
    let query_bytes = gen_bytes(query_seed, query_count);

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if let Err(e) = write_header(&mut out, dims, &dtype, tree_count, query_count) {
        eprintln!("spatial-bench-dataset: header failed: {e}");
        std::process::exit(1);
    }
    if let Err(e) = write_points(&mut out, &tree_bytes) {
        eprintln!("spatial-bench-dataset: tree points failed: {e}");
        std::process::exit(1);
    }
    if let Err(e) = write_points(&mut out, &query_bytes) {
        eprintln!("spatial-bench-dataset: query points failed: {e}");
        std::process::exit(1);
    }
}
