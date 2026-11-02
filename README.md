# jetson_jpeg

[![License](https://img.shields.io/badge/license-MIT%2FApache-blue.svg)](https://github.com/IntelligentRoboticsLab/jetson_jpeg#license)
[![Crates.io](https://img.shields.io/crates/v/jetson_jpeg.svg)](https://crates.io/crates/jetson_jpeg)
[![Docs](https://docs.rs/jetson_jpeg/badge.svg)](https://docs.rs/jetson_jpeg/latest/jetson_jpeg/)

`jetson_jpeg` encodes NV12 frames to JPEG on the NVJPG engine of NVIDIA Jetson
modules. It drives L4T's `libnvjpeg`, NVIDIA's libjpeg with hardware
extensions, and hands it frames as `NvBufSurface` hardware buffers. Encoding
on the NVJPG engine leaves the CPU and the video encoder free.

The libraries are loaded at runtime, so the crate builds on any Unix host,
including CI machines and laptops. Opening an encoder returns an error where
the libraries are missing or the L4T release is not supported.

## Example

```rust
use jetson_jpeg::{JpegEncoder, JpegSettings, Layout, Nv12};

let mut encoder = JpegEncoder::open(JpegSettings {
    width: 1280,
    height: 720,
    quality: 90,
    layout: Layout::Pitch,
})?;

// `frame` holds 720 rows of luma, then 360 rows of interleaved chroma.
let jpeg: Vec<u8> = encoder.encode(Nv12 { data: &frame, stride: 1280 })?;
```

Each frame is copied into a pitch-linear hardware buffer that the engine
encodes from. With `Layout::BlockLinear` the frame is first converted to
NVIDIA's tiled layout on the VIC, which the engine reads natively.

`examples/encode_file.rs` encodes a raw NV12 file and reports the encode time:

```bash
cargo run --release --example encode_file -- frame.nv12 1280 720 frame.jpg 90
```

## Requirements

- A Jetson Orin module on JetPack 6 (L4T R36). The struct layouts are those of
  R36's Multimedia API headers, so opening an encoder checks
  `/etc/nv_tegra_release` and refuses other releases.
- `libnvjpeg.so`, `libnvbufsurface.so` and `libnvbufsurftransform.so`, which
  ship with L4T in `/usr/lib/aarch64-linux-gnu/nvidia/`. That directory is
  searched first, because CUDA's nvJPEG library has the same file name but a
  different API.

## Errors

Failures come back as a `jetson_jpeg::Error`: an unsupported platform, missing
libraries, invalid settings or frames, a failed hardware buffer call, and a
frame libnvjpeg refuses to start. The settings are checked before libjpeg sees
them, so its fatal error handler is only reached by a fault in NVIDIA's library
or the hardware. That handler must not return to libjpeg and cannot unwind into
Rust, so it panics with libjpeg's message, which aborts the process. libjpeg's
warnings are logged through `tracing`.

## Limitations

- Only L4T R36 is supported. JetPack 7 widened `NvBufSurfaceParams` and
  `jpeg_compress_struct`, and JetPack 5 is untested.
- An encoder handles one frame size. Open one encoder per size.

## License

Licensed under either the [Apache License, Version 2.0](LICENSE-APACHE) or the
[MIT License](LICENSE-MIT), at your option.
