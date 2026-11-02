//! Encodes a raw NV12 frame to a JPEG on the NVJPG engine:
//!
//! ```bash
//! cargo run --release --example encode_file -- frame.nv12 1280 720 frame.jpg [quality]
//! ```
//!
//! The input holds `height` rows of luma then `height / 2` rows of interleaved
//! chroma, each `width` bytes long.

use std::time::Instant;

use anyhow::{Context, Result, bail};
use jetson_jpeg::{JpegEncoder, JpegSettings, Layout, Nv12};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, width, height, output, rest @ ..] = args.as_slice() else {
        bail!("usage: encode_file <input.nv12> <width> <height> <output.jpg> [quality]");
    };
    let width: u32 = width.parse().context("width")?;
    let height: u32 = height.parse().context("height")?;
    let quality: u8 = match rest.first() {
        Some(quality) => quality.parse().context("quality")?,
        None => 90,
    };

    let frame = std::fs::read(input).with_context(|| format!("reading {input}"))?;
    let mut encoder = JpegEncoder::open(JpegSettings {
        width,
        height,
        quality,
        layout: Layout::Pitch,
    })?;

    let start = Instant::now();
    let jpeg = encoder.encode(Nv12 {
        data: &frame,
        stride: width as usize,
    })?;
    let elapsed = start.elapsed();

    std::fs::write(output, &jpeg).with_context(|| format!("writing {output}"))?;
    println!(
        "{width}x{height} at quality {quality}: {} bytes in {:.2} ms",
        jpeg.len(),
        elapsed.as_secs_f64() * 1e3
    );
    Ok(())
}
