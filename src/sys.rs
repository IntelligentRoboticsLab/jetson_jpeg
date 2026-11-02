//! The libjpeg and `NvBufSurface` structs and functions the encoder uses, and the
//! runtime-loaded Jetson libraries that implement them.
//!
//! `libnvjpeg` is NVIDIA's libjpeg-8b built with `TEGRA_ACCELERATE`, which adds
//! fields to the libjpeg structs, so `jpeg_compress_struct` is kept as opaque
//! storage written at the offsets the encoder needs. Layouts and offsets are the
//! 64-bit Linux ones from Jetson Linux R36's Multimedia API headers
//! (`libjpeg-8b/jpeglib.h`, `nvbufsurface.h`, `nvbufsurftransform.h`), checked by
//! the assertions below. Other releases change them, so [`Api::load`] only
//! loads on R36.

#![allow(clippy::struct_field_names)]

use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::mem::{offset_of, size_of};

use libloading::Library;
use libloading::os::unix::Library as UnixLibrary;

use crate::{Error, Result};

pub const JPEG_LIB_VERSION: c_int = 80;
/// libjpeg's `boolean` is an `int`.
pub const TRUE: c_int = 1;
pub const FALSE: c_int = 0;
pub const JCS_YCBCR: c_int = 3;
/// `JMSG_LENGTH_MAX`: the buffer `format_message` writes into.
pub const MESSAGE_LENGTH: usize = 200;
/// `JPEG_MAX_DIMENSION`: the largest width or height libjpeg accepts.
pub const MAX_DIMENSION: u32 = 65500;

/// `sizeof(struct jpeg_compress_struct)`, which `jpeg_CreateCompress` checks.
pub const COMPRESS_STRUCT_SIZE: usize = 728;

/// Offsets of the `jpeg_compress_struct` fields the encoder sets.
pub mod compress {
    pub const ERR: usize = 0;
    pub const IS_VENDOR_BUF: usize = 92;
    pub const FD: usize = 112;
    pub const IMAGE_WIDTH: usize = 128;
    pub const IMAGE_HEIGHT: usize = 132;
    pub const INPUT_COMPONENTS: usize = 136;
    pub const IN_COLOR_SPACE: usize = 140;
    pub const RAW_DATA_IN: usize = 368;
}

pub const NVBUF_COLOR_FORMAT_NV12: u32 = 6;
pub const NVBUF_LAYOUT_PITCH: u32 = 0;
pub const NVBUF_LAYOUT_BLOCK_LINEAR: u32 = 1;
pub const NVBUF_MEM_SURFACE_ARRAY: u32 = 4;
pub const NVBUF_MAP_WRITE: u32 = 1;
pub const NVBUF_SURFACE_TAG_VIDEO_CONVERT: u32 = 0xf01;
pub const NVBUF_MAX_PLANES: usize = 4;
pub const NVBUFSURF_TRANSFORM_FILTER: u32 = 1 << 2;

/// `struct jpeg_compress_struct`, as bytes.
#[repr(C, align(8))]
pub struct CompressStruct([u8; COMPRESS_STRUCT_SIZE]);

impl CompressStruct {
    pub fn zeroed() -> Box<Self> {
        Box::new(Self([0; COMPRESS_STRUCT_SIZE]))
    }

    pub fn as_mut_ptr(&mut self) -> *mut c_void {
        self.0.as_mut_ptr().cast()
    }

    /// Writes the field at `offset`, which holds a `T`.
    pub fn set<T: Copy>(&mut self, offset: usize, value: T) {
        assert!(offset + size_of::<T>() <= COMPRESS_STRUCT_SIZE);
        // SAFETY: in bounds per the assertion; the field is plain data.
        unsafe {
            self.0
                .as_mut_ptr()
                .add(offset)
                .cast::<T>()
                .write_unaligned(value);
        }
    }
}

/// `struct jpeg_error_mgr`.
#[repr(C)]
pub struct ErrorMgr {
    pub error_exit: Option<unsafe extern "C" fn(*mut c_void)>,
    pub emit_message: Option<unsafe extern "C" fn(*mut c_void, c_int)>,
    pub output_message: Option<unsafe extern "C" fn(*mut c_void)>,
    pub format_message: Option<unsafe extern "C" fn(*mut c_void, *mut c_char)>,
    pub reset_error_mgr: Option<unsafe extern "C" fn(*mut c_void)>,
    pub msg_code: c_int,
    /// The union of `int[8]` and `char[80]` message parameters.
    pub msg_parm: [u8; 80],
    pub trace_level: c_int,
    pub num_warnings: c_long,
    pub jpeg_message_table: *const *const c_char,
    pub last_jpeg_message: c_int,
    pub addon_message_table: *const *const c_char,
    pub first_addon_message: c_int,
    pub last_addon_message: c_int,
}

impl ErrorMgr {
    pub fn zeroed() -> Box<Self> {
        Box::new(Self {
            error_exit: None,
            emit_message: None,
            output_message: None,
            format_message: None,
            reset_error_mgr: None,
            msg_code: 0,
            msg_parm: [0; 80],
            trace_level: 0,
            num_warnings: 0,
            jpeg_message_table: std::ptr::null(),
            last_jpeg_message: 0,
            addon_message_table: std::ptr::null(),
            first_addon_message: 0,
            last_addon_message: 0,
        })
    }
}

/// `NvBufSurfaceCreateParams`.
#[repr(C)]
#[derive(Default)]
pub struct SurfaceCreateParams {
    pub gpu_id: u32,
    pub width: u32,
    pub height: u32,
    pub size: u32,
    pub is_contiguous: bool,
    pub color_format: u32,
    pub layout: u32,
    pub mem_type: u32,
}

/// `NvBufSurfaceAllocateParams`.
#[repr(C)]
pub struct SurfaceAllocateParams {
    pub params: SurfaceCreateParams,
    pub display_scan_format: u32,
    pub chroma_loc_horiz: u8,
    pub chroma_loc_vert: u8,
    pub memtag: u32,
    pub disable_pitch_padding: bool,
    pub reserved_param: u32,
    pub reserved: [*mut c_void; 3],
}

/// `NvBufSurfacePlaneParams`.
#[repr(C)]
pub struct PlaneParams {
    pub num_planes: u32,
    pub width: [u32; NVBUF_MAX_PLANES],
    pub height: [u32; NVBUF_MAX_PLANES],
    pub pitch: [u32; NVBUF_MAX_PLANES],
    pub offset: [u32; NVBUF_MAX_PLANES],
    pub psize: [u32; NVBUF_MAX_PLANES],
    pub bytes_per_pix: [u32; NVBUF_MAX_PLANES],
    pub reserved: [*mut c_void; 4 * NVBUF_MAX_PLANES],
}

/// `NvBufSurfaceMappedAddr`.
#[repr(C)]
pub struct MappedAddr {
    pub addr: [*mut c_void; NVBUF_MAX_PLANES],
    pub egl_image: *mut c_void,
    pub reserved: [*mut c_void; 4],
}

/// `NvBufSurfaceParams`: one buffer of a surface batch.
#[repr(C)]
pub struct SurfaceParams {
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub color_format: u32,
    pub layout: u32,
    /// The DMABUF fd of a `NVBUF_MEM_SURFACE_ARRAY` buffer.
    pub buffer_desc: u64,
    pub data_size: u32,
    pub data_ptr: *mut c_void,
    pub plane_params: PlaneParams,
    pub mapped_addr: MappedAddr,
    pub paramex: *mut c_void,
    pub reserved: [*mut c_void; 3],
}

/// `NvBufSurface`: a batch of buffers.
#[repr(C)]
pub struct Surface {
    pub gpu_id: u32,
    pub batch_size: u32,
    pub num_filled: u32,
    pub is_contiguous: bool,
    pub mem_type: u32,
    pub surface_list: *mut SurfaceParams,
    pub reserved: [*mut c_void; 4],
}

/// `NvBufSurfTransformRect`.
#[repr(C)]
pub struct TransformRect {
    pub top: u32,
    pub left: u32,
    pub width: u32,
    pub height: u32,
}

/// `NvBufSurfTransformParams`.
#[repr(C)]
pub struct TransformParams {
    pub transform_flag: u32,
    pub transform_flip: u32,
    pub transform_filter: u32,
    pub src_rect: *mut TransformRect,
    pub dst_rect: *mut TransformRect,
}

type Cinfo = *mut c_void;

const _: () = {
    assert!(size_of::<ErrorMgr>() == 168);
    assert!(offset_of!(ErrorMgr, msg_code) == 40);
    assert!(offset_of!(ErrorMgr, trace_level) == 124);
    assert!(offset_of!(ErrorMgr, num_warnings) == 128);
    assert!(offset_of!(ErrorMgr, jpeg_message_table) == 136);
    assert!(offset_of!(ErrorMgr, last_addon_message) == 164);
    assert!(size_of::<SurfaceCreateParams>() == 32);
    assert!(offset_of!(SurfaceCreateParams, is_contiguous) == 16);
    assert!(offset_of!(SurfaceCreateParams, color_format) == 20);
    assert!(offset_of!(SurfaceCreateParams, mem_type) == 28);
    assert!(size_of::<SurfaceAllocateParams>() == 80);
    assert!(offset_of!(SurfaceAllocateParams, display_scan_format) == 32);
    assert!(offset_of!(SurfaceAllocateParams, chroma_loc_horiz) == 36);
    assert!(offset_of!(SurfaceAllocateParams, memtag) == 40);
    assert!(offset_of!(SurfaceAllocateParams, disable_pitch_padding) == 44);
    assert!(offset_of!(SurfaceAllocateParams, reserved) == 56);
    assert!(size_of::<PlaneParams>() == 232);
    assert!(offset_of!(PlaneParams, height) == 20);
    assert!(offset_of!(PlaneParams, pitch) == 36);
    assert!(size_of::<MappedAddr>() == 72);
    assert!(size_of::<SurfaceParams>() == 384);
    assert!(offset_of!(SurfaceParams, buffer_desc) == 24);
    assert!(offset_of!(SurfaceParams, plane_params) == 48);
    assert!(offset_of!(SurfaceParams, mapped_addr) == 280);
    assert!(offset_of!(SurfaceParams, paramex) == 352);
    assert!(size_of::<Surface>() == 64);
    assert!(offset_of!(Surface, num_filled) == 8);
    assert!(offset_of!(Surface, surface_list) == 24);
    assert!(size_of::<TransformParams>() == 32);
    assert!(offset_of!(TransformParams, src_rect) == 16);
};

/// The functions the encoder calls, and the libraries they point into.
pub struct Api {
    pub jpeg_std_error: unsafe extern "C" fn(*mut ErrorMgr) -> *mut ErrorMgr,
    pub jpeg_create_compress: unsafe extern "C" fn(Cinfo, c_int, usize),
    pub jpeg_destroy_compress: unsafe extern "C" fn(Cinfo),
    pub jpeg_abort_compress: unsafe extern "C" fn(Cinfo),
    pub jpeg_suppress_tables: unsafe extern "C" fn(Cinfo, c_int),
    pub jpeg_mem_dest: unsafe extern "C" fn(Cinfo, *mut *mut u8, *mut c_ulong),
    pub jpeg_set_defaults: unsafe extern "C" fn(Cinfo),
    pub jpeg_set_quality: unsafe extern "C" fn(Cinfo, c_int, c_int),
    pub jpeg_set_hardware_acceleration_parameters_enc:
        unsafe extern "C" fn(Cinfo, c_int, c_uint, c_uint, c_uint),
    pub jpeg_start_compress: unsafe extern "C" fn(Cinfo, c_int),
    pub jpeg_write_raw_data: unsafe extern "C" fn(Cinfo, *mut c_void, c_uint) -> c_uint,
    pub jpeg_finish_compress: unsafe extern "C" fn(Cinfo),
    pub surface_allocate:
        unsafe extern "C" fn(*mut *mut Surface, u32, *mut SurfaceAllocateParams) -> c_int,
    pub surface_destroy: unsafe extern "C" fn(*mut Surface) -> c_int,
    pub surface_map: unsafe extern "C" fn(*mut Surface, c_int, c_int, u32) -> c_int,
    pub surface_unmap: unsafe extern "C" fn(*mut Surface, c_int, c_int) -> c_int,
    pub surface_sync_for_device: unsafe extern "C" fn(*mut Surface, c_int, c_int) -> c_int,
    pub surface_transform:
        unsafe extern "C" fn(*mut Surface, *mut Surface, *mut TransformParams) -> c_int,
    // The function pointers above point into these.
    _jpeg: Library,
    _surface: Library,
    _transform: Library,
}

/// The L4T libnvjpeg comes first: CUDA's nvJPEG is also called `libnvjpeg.so` and
/// may be on the library path, but has a different API.
const JPEG_LIBRARIES: [&str; 2] = [
    "/usr/lib/aarch64-linux-gnu/nvidia/libnvjpeg.so",
    "libnvjpeg.so",
];
const SURFACE_LIBRARIES: [&str; 2] = [
    "/usr/lib/aarch64-linux-gnu/nvidia/libnvbufsurface.so",
    "libnvbufsurface.so",
];
const TRANSFORM_LIBRARIES: [&str; 2] = [
    "/usr/lib/aarch64-linux-gnu/nvidia/libnvbufsurftransform.so",
    "libnvbufsurftransform.so",
];

/// Holds the L4T release, as in `# R36 (release), REVISION: 4.3, ...`.
const RELEASE_FILE: &str = "/etc/nv_tegra_release";
/// The release whose headers the layouts above come from.
const SUPPORTED_RELEASE: &str = "R36";

impl Api {
    /// Loads the libraries, on an L4T R36 Jetson only.
    pub fn load() -> Result<Self> {
        check_release()?;
        let jpeg = load_first(&JPEG_LIBRARIES)?;
        let surface = load_first(&SURFACE_LIBRARIES)?;
        let transform = load_first(&TRANSFORM_LIBRARIES)?;
        // SAFETY: the symbol types match NVIDIA's declarations (jpeglib.h,
        // nvbufsurface.h, nvbufsurftransform.h).
        unsafe {
            Ok(Self {
                jpeg_std_error: symbol(&jpeg, "jpeg_std_error")?,
                jpeg_create_compress: symbol(&jpeg, "jpeg_CreateCompress")?,
                jpeg_destroy_compress: symbol(&jpeg, "jpeg_destroy_compress")?,
                jpeg_abort_compress: symbol(&jpeg, "jpeg_abort_compress")?,
                jpeg_suppress_tables: symbol(&jpeg, "jpeg_suppress_tables")?,
                jpeg_mem_dest: symbol(&jpeg, "jpeg_mem_dest")?,
                jpeg_set_defaults: symbol(&jpeg, "jpeg_set_defaults")?,
                jpeg_set_quality: symbol(&jpeg, "jpeg_set_quality")?,
                jpeg_set_hardware_acceleration_parameters_enc: *jpeg
                    .get(b"jpeg_set_hardware_acceleration_parameters_enc")
                    .map_err(Error::NotTegraLibjpeg)?,
                jpeg_start_compress: symbol(&jpeg, "jpeg_start_compress")?,
                jpeg_write_raw_data: symbol(&jpeg, "jpeg_write_raw_data")?,
                jpeg_finish_compress: symbol(&jpeg, "jpeg_finish_compress")?,
                surface_allocate: symbol(&surface, "NvBufSurfaceAllocate")?,
                surface_destroy: symbol(&surface, "NvBufSurfaceDestroy")?,
                surface_map: symbol(&surface, "NvBufSurfaceMap")?,
                surface_unmap: symbol(&surface, "NvBufSurfaceUnMap")?,
                surface_sync_for_device: symbol(&surface, "NvBufSurfaceSyncForDevice")?,
                surface_transform: symbol(&transform, "NvBufSurfTransform")?,
                _jpeg: jpeg,
                _surface: surface,
                _transform: transform,
            })
        }
    }
}

fn check_release() -> Result<()> {
    let contents = std::fs::read_to_string(RELEASE_FILE).map_err(Error::NotJetson)?;
    let release = contents
        .trim_start_matches('#')
        .split_whitespace()
        .next()
        .unwrap_or_default();
    if release == SUPPORTED_RELEASE {
        Ok(())
    } else {
        Err(Error::UnsupportedRelease {
            release: release.to_owned(),
        })
    }
}

/// The function `name` in `library`.
///
/// # Safety
/// `T` must be the function's type.
unsafe fn symbol<T: Copy>(library: &Library, name: &'static str) -> Result<T> {
    // SAFETY: the caller names the function's type.
    unsafe { library.get::<T>(name.as_bytes()) }
        .map(|symbol| *symbol)
        .map_err(|source| Error::Load { name, source })
}

fn load_first(candidates: &[&'static str]) -> Result<Library> {
    let mut last_error = None;
    for name in candidates {
        // NVIDIA's libraries register thread-exit destructors, which crash a thread
        // that exits after the library was unloaded, so they are never unloaded.
        // SAFETY: NVIDIA's libraries run no initialisers with preconditions.
        match unsafe {
            UnixLibrary::open(
                Some(name),
                libloading::os::unix::RTLD_NOW | libc::RTLD_NODELETE,
            )
        } {
            Ok(library) => return Ok(library.into()),
            Err(error) => last_error = Some(error),
        }
    }
    Err(Error::Load {
        name: candidates[candidates.len() - 1],
        source: last_error.expect("there are library candidates"),
    })
}
