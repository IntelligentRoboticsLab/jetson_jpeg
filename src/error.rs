use std::io;

/// Why opening a [`JpegEncoder`](crate::JpegEncoder) or encoding a frame failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// `/etc/nv_tegra_release` could not be read, so this is not a Jetson.
    #[error("not a Jetson: cannot read /etc/nv_tegra_release")]
    NotJetson(#[source] io::Error),
    /// The Jetson runs an L4T release whose struct layouts this crate does not know.
    #[error("unsupported L4T release {release:?}, only R36 is supported")]
    UnsupportedRelease { release: String },
    /// One of NVIDIA's libraries, or a function in it, could not be loaded.
    #[error("failed to load {name}")]
    Load {
        name: &'static str,
        #[source]
        source: libloading::Error,
    },
    /// The loaded `libnvjpeg.so` lacks L4T's hardware extensions, as CUDA's nvJPEG does.
    #[error("libnvjpeg.so is not L4T's libjpeg")]
    NotTegraLibjpeg(#[source] libloading::Error),
    /// The width and height must be even, and from 2 to libjpeg's limit of 65500.
    #[error("invalid frame size {width}x{height}, expected even sizes from 2 to 65500")]
    InvalidSize { width: u32, height: u32 },
    /// JPEG quality runs from 1 to 100.
    #[error("invalid JPEG quality {quality}, expected 1-100")]
    InvalidQuality { quality: u8 },
    /// A frame's rows are shorter than the encoder's width.
    #[error("frame stride {stride} is less than the width {width}")]
    StrideTooSmall { stride: usize, width: u32 },
    /// A frame holds fewer bytes than its stride and the encoder's height need.
    #[error("frame has {len} bytes, but needs {needed} at stride {stride}")]
    FrameTooShort {
        len: usize,
        needed: usize,
        stride: usize,
    },
    /// A hardware buffer (`NvBufSurface`) call failed. `status` is what it
    /// returned, which is 0 when it reported success without a usable buffer.
    #[error("{operation} failed with status {status}")]
    Surface {
        operation: &'static str,
        status: i32,
    },
    /// libnvjpeg failed to encode, with its message.
    #[error("libnvjpeg: {message}")]
    Jpeg { message: String },
    /// libnvjpeg finished a frame without writing any data.
    #[error("libnvjpeg produced no data")]
    EmptyOutput,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
