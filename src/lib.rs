//! Hardware JPEG encoding on NVIDIA Jetson (Orin) through L4T's libnvjpeg, which
//! runs on the NVJPG engine, separate from the video encoder.
//!
//! libnvjpeg is NVIDIA's libjpeg with hardware extensions; frames go to it as
//! `NvBufSurface` hardware buffers. It, `libnvbufsurface` and
//! `libnvbufsurftransform` are loaded at runtime: this crate builds on any Unix
//! host and only fails to open a [`JpegEncoder`] where they are missing.
//!
//! The struct layouts are those of L4T R36, so [`JpegEncoder::open`]
//! checks `/etc/nv_tegra_release` and refuses any other release.
//!
//! ```no_run
//! use jetson_jpeg::{JpegEncoder, JpegSettings, Layout, Nv12};
//!
//! let (width, height) = (1280, 720);
//! let frame = vec![128_u8; width * height * 3 / 2];
//! let mut encoder = JpegEncoder::open(JpegSettings {
//!     width: width as u32,
//!     height: height as u32,
//!     quality: 90,
//!     layout: Layout::Pitch,
//! })?;
//! let jpeg: Vec<u8> = encoder.encode(Nv12 { data: &frame, stride: width })?;
//! # Ok::<(), jetson_jpeg::Error>(())
//! ```

#[cfg(not(unix))]
compile_error!("jetson_jpeg loads NVIDIA's L4T libraries and only builds on Unix");

mod encoder;
mod error;
mod surface;
mod sys;

use std::fmt;

pub use encoder::{JpegEncoder, JpegSettings};
pub use error::{Error, Result};
pub use surface::Layout;

/// An NV12 frame: `height` rows of luma then `height / 2` rows of interleaved
/// chroma, each `stride` bytes apart.
#[derive(Clone, Copy)]
pub struct Nv12<'a> {
    pub data: &'a [u8],
    pub stride: usize,
}

impl fmt::Debug for Nv12<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Nv12")
            .field("len", &self.data.len())
            .field("stride", &self.stride)
            .finish()
    }
}
