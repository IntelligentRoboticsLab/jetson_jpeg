//! One JPEG encoding session: NV12 frames in, one JPEG per frame out.
//!
//! Each frame is copied into a hardware buffer, which libnvjpeg encodes on the
//! NVJPG engine straight from its fd. `encode` returns once the JPEG is done.

use std::ffi::{CStr, c_char, c_ulong, c_void};
use std::fmt;
use std::sync::Arc;

use tracing::warn;

use crate::surface::{Layout, Nv12Surface};
use crate::sys::{self, Api, CompressStruct, ErrorMgr, compress};
use crate::{Error, Nv12, Result};

/// The frame size, quality and buffer layout of a [`JpegEncoder`].
#[derive(Clone, Copy, Debug)]
pub struct JpegSettings {
    /// Frame width in pixels, even and at most 65500.
    pub width: u32,
    /// Frame height in pixels, even and at most 65500.
    pub height: u32,
    /// 1-100, higher is better quality and more bytes.
    pub quality: u8,
    /// The layout of the buffer the engine reads. A block-linear frame is first
    /// converted from a pitch-linear copy on the VIC.
    pub layout: Layout,
}

/// One JPEG encoding session on the NVJPG engine, for frames of one size.
pub struct JpegEncoder {
    api: Arc<Api>,
    settings: JpegSettings,
    cinfo: Box<CompressStruct>,
    /// The session's error manager, which `cinfo` points at.
    error_mgr: Box<ErrorMgr>,
    input: Nv12Surface,
    block_linear: Option<Nv12Surface>,
    /// The output buffer libjpeg writes each JPEG into (`malloc`ed, as libjpeg may
    /// replace it with a larger one of its own).
    output: *mut u8,
    output_capacity: usize,
}

// SAFETY: the session's libjpeg and surface state is only touched through `&mut self`.
unsafe impl Send for JpegEncoder {}

impl JpegEncoder {
    /// Loads NVIDIA's libraries and sets up a session with its hardware buffers.
    /// Fails off a Jetson, on an L4T release other than R36, or where the
    /// libraries are missing.
    pub fn open(settings: JpegSettings) -> Result<Self> {
        let valid_dimension =
            |size: u32| size > 0 && size <= sys::MAX_DIMENSION && size.is_multiple_of(2);
        if !valid_dimension(settings.width) || !valid_dimension(settings.height) {
            return Err(Error::InvalidSize {
                width: settings.width,
                height: settings.height,
            });
        }
        if !(1..=100).contains(&settings.quality) {
            return Err(Error::InvalidQuality {
                quality: settings.quality,
            });
        }
        let api = Arc::new(Api::load()?);
        let input = Nv12Surface::allocate(
            Arc::clone(&api),
            settings.width,
            settings.height,
            Layout::Pitch,
        )?;
        let block_linear = match settings.layout {
            Layout::Pitch => None,
            Layout::BlockLinear => Some(Nv12Surface::allocate(
                Arc::clone(&api),
                settings.width,
                settings.height,
                Layout::BlockLinear,
            )?),
        };

        let mut error_mgr = ErrorMgr::zeroed();
        let mut cinfo = CompressStruct::zeroed();
        // SAFETY: `error_mgr` and `cinfo` are boxed, so the pointers libjpeg keeps
        // stay valid for the session; NVIDIA's encoder sets them up the same way.
        // The settings were checked above and the release by `Api::load`, so
        // creating the session does not fail.
        unsafe {
            let error_mgr_ptr = (api.jpeg_std_error)(&raw mut *error_mgr);
            error_mgr.error_exit = Some(error_exit);
            error_mgr.output_message = Some(output_message);
            cinfo.set(compress::ERR, error_mgr_ptr);
            (api.jpeg_create_compress)(
                cinfo.as_mut_ptr(),
                sys::JPEG_LIB_VERSION,
                sys::COMPRESS_STRUCT_SIZE,
            );
            (api.jpeg_suppress_tables)(cinfo.as_mut_ptr(), sys::TRUE);
        }

        // An NV12 frame's size is more than any JPEG of it; NVIDIA's sample uses the same.
        let output_capacity = settings.width as usize * settings.height as usize * 3 / 2;
        // SAFETY: plain allocation, released in `drop`.
        let output = unsafe { libc::malloc(output_capacity) }.cast::<u8>();
        if output.is_null() {
            std::alloc::handle_alloc_error(
                std::alloc::Layout::array::<u8>(output_capacity).expect("the capacity fits"),
            );
        }

        Ok(Self {
            api,
            settings,
            cinfo,
            error_mgr,
            input,
            block_linear,
            output,
            output_capacity,
        })
    }

    /// The settings the encoder was opened with.
    #[must_use]
    pub fn settings(&self) -> &JpegSettings {
        &self.settings
    }

    /// Encodes one frame, returning the JPEG file's bytes.
    pub fn encode(&mut self, frame: Nv12<'_>) -> Result<Vec<u8>> {
        let mut jpeg = Vec::new();
        self.encode_into(frame, &mut jpeg)?;
        Ok(jpeg)
    }

    /// Encodes one frame into `jpeg`, replacing its contents with the JPEG file's
    /// bytes. Reusing one `Vec` saves an allocation per frame.
    pub fn encode_into(&mut self, frame: Nv12<'_>, jpeg: &mut Vec<u8>) -> Result<()> {
        self.input.write(frame)?;
        let fd = match &self.block_linear {
            Some(block_linear) => {
                self.input.transform_into(block_linear)?;
                block_linear.fd()
            }
            None => self.input.fd(),
        };

        let api = &self.api;
        let cinfo = self.cinfo.as_mut_ptr();
        let mut output = self.output;
        let mut output_size = c_ulong::try_from(self.output_capacity).unwrap_or(c_ulong::MAX);
        let capacity = u32::try_from(self.output_capacity).unwrap_or(u32::MAX);
        // The same calls, in the same order, as NVIDIA's NvJPEGEncoder::encodeFromFd.
        // SAFETY: `cinfo` is a live compress session; `output` and `output_size`
        // outlive it, and libjpeg writes at most `output_size` bytes into `output`
        // before switching to a buffer of its own.
        unsafe {
            self.error_mgr.msg_code = 0;
            (api.jpeg_mem_dest)(cinfo, &raw mut output, &raw mut output_size);
            self.cinfo.set(compress::FD, fd);
            self.cinfo.set(compress::IS_VENDOR_BUF, sys::TRUE);
            self.cinfo.set(compress::IMAGE_WIDTH, self.settings.width);
            self.cinfo.set(compress::IMAGE_HEIGHT, self.settings.height);
            self.cinfo.set(compress::INPUT_COMPONENTS, 3_i32);
            self.cinfo.set(compress::RAW_DATA_IN, sys::TRUE);
            self.cinfo.set(compress::IN_COLOR_SPACE, sys::JCS_YCBCR);
            (api.jpeg_set_defaults)(cinfo);
            (api.jpeg_set_quality)(cinfo, i32::from(self.settings.quality), sys::TRUE);
            (api.jpeg_set_hardware_acceleration_parameters_enc)(cinfo, sys::TRUE, capacity, 0, 0);
            (api.jpeg_start_compress)(cinfo, sys::FALSE);
            // As in NVIDIA's encoder, a message left by jpeg_start_compress fails the frame.
            if self.error_mgr.msg_code != 0 {
                let message = format_message(cinfo);
                (api.jpeg_abort_compress)(cinfo);
                return Err(Error::Jpeg { message });
            }
            (api.jpeg_write_raw_data)(cinfo, std::ptr::null_mut(), 0);
            (api.jpeg_finish_compress)(cinfo);
        }

        let length = usize::try_from(output_size).unwrap_or(0);
        let result = if output.is_null() || length == 0 {
            Err(Error::EmptyOutput)
        } else {
            jpeg.clear();
            // SAFETY: libjpeg wrote `length` bytes at `output`.
            jpeg.extend_from_slice(unsafe { std::slice::from_raw_parts(output, length) });
            Ok(())
        };
        if output != self.output {
            // SAFETY: libjpeg `malloc`ed a larger buffer and left it to us.
            unsafe { libc::free(output.cast()) };
        }
        result
    }
}

impl fmt::Debug for JpegEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JpegEncoder")
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl Drop for JpegEncoder {
    fn drop(&mut self) {
        // SAFETY: the session and buffer are released once.
        unsafe {
            (self.api.jpeg_destroy_compress)(self.cinfo.as_mut_ptr());
            libc::free(self.output.cast());
        }
    }
}

/// libjpeg's message for its most recent error or warning.
///
/// # Safety
/// `cinfo` must be a live session whose error manager is set.
unsafe fn format_message(cinfo: *mut c_void) -> String {
    let mut buffer = [0 as c_char; sys::MESSAGE_LENGTH];
    // SAFETY: the error manager pointer is the session's first field.
    unsafe {
        let error_mgr = cinfo.cast::<*mut ErrorMgr>().read();
        match (*error_mgr).format_message {
            Some(format) => format(cinfo, buffer.as_mut_ptr()),
            None => return "unknown libjpeg error".to_owned(),
        }
        CStr::from_ptr(buffer.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

/// libjpeg's fatal error handler, which must not return to libjpeg. With the
/// settings checked in `open`, only a fault in NVIDIA's library or the hardware
/// gets here. A panic cannot unwind out of an `extern "C"` function, so this
/// prints the message and aborts the process.
unsafe extern "C" fn error_exit(cinfo: *mut c_void) {
    // SAFETY: libjpeg passes the session that failed.
    let message = unsafe { format_message(cinfo) };
    panic!("libnvjpeg fatal error: {message}");
}

/// libjpeg's warnings, which it reports through the session's `output_message`.
unsafe extern "C" fn output_message(cinfo: *mut c_void) {
    // SAFETY: libjpeg passes the session the message belongs to.
    let message = unsafe { format_message(cinfo) };
    warn!("libnvjpeg: {message}");
}
