//! An NV12 hardware buffer (`NvBufSurface`) the JPEG engine reads from.

use std::ffi::c_int;
use std::sync::Arc;

use crate::sys::{self, Api, Surface};
use crate::{Error, Nv12, Result};

/// How the buffer's pixels are arranged in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Row after row, which the CPU can write.
    Pitch,
    /// NVIDIA's tiled layout, which the hardware engines read natively.
    BlockLinear,
}

pub struct Nv12Surface {
    api: Arc<Api>,
    surface: *mut Surface,
    width: u32,
    height: u32,
    /// The buffer's DMABUF fd, by which libnvjpeg finds it.
    fd: c_int,
    /// Whether the planes are mapped for the CPU to write.
    mapped: bool,
}

impl Nv12Surface {
    /// Allocates the buffer; a pitch-linear one is also mapped for [`Self::write`].
    pub fn allocate(api: Arc<Api>, width: u32, height: u32, layout: Layout) -> Result<Self> {
        let mut params = sys::SurfaceAllocateParams {
            params: sys::SurfaceCreateParams {
                width,
                height,
                color_format: sys::NVBUF_COLOR_FORMAT_NV12,
                layout: match layout {
                    Layout::Pitch => sys::NVBUF_LAYOUT_PITCH,
                    Layout::BlockLinear => sys::NVBUF_LAYOUT_BLOCK_LINEAR,
                },
                mem_type: sys::NVBUF_MEM_SURFACE_ARRAY,
                ..Default::default()
            },
            display_scan_format: 0,
            chroma_loc_horiz: 0,
            chroma_loc_vert: 0,
            memtag: sys::NVBUF_SURFACE_TAG_VIDEO_CONVERT,
            disable_pitch_padding: false,
            reserved_param: 0,
            reserved: [std::ptr::null_mut(); 3],
        };
        let mut surface: *mut Surface = std::ptr::null_mut();
        // SAFETY: `params` is a valid allocate request; `surface` receives the new batch.
        let status = unsafe { (api.surface_allocate)(&raw mut surface, 1, &raw mut params) };
        if status != 0 || surface.is_null() {
            return Err(Error::Surface {
                operation: "NvBufSurfaceAllocate",
                status,
            });
        }
        // SAFETY: a successful allocation returns a valid one-buffer batch.
        let buffer_desc = unsafe {
            (*surface).num_filled = 1;
            (*(*surface).surface_list).buffer_desc
        };
        let mut allocated = Self {
            api,
            surface,
            width,
            height,
            fd: c_int::try_from(buffer_desc).unwrap_or(-1),
            mapped: false,
        };
        if allocated.fd < 0 {
            return Err(Error::Surface {
                operation: "NvBufSurfaceAllocate",
                status,
            });
        }
        if layout == Layout::Pitch {
            allocated.map()?;
        }
        Ok(allocated)
    }

    fn map(&mut self) -> Result<()> {
        // SAFETY: `surface` is a live batch; plane -1 maps every plane.
        let status = unsafe { (self.api.surface_map)(self.surface, 0, -1, sys::NVBUF_MAP_WRITE) };
        if status != 0 {
            return Err(Error::Surface {
                operation: "NvBufSurfaceMap",
                status,
            });
        }
        self.mapped = true;
        let width = self.width as usize;
        let params = self.params();
        let planes_usable = (0..2).all(|plane| {
            !params.mapped_addr.addr[plane].is_null()
                && params.plane_params.pitch[plane] as usize >= width
        });
        if !planes_usable {
            return Err(Error::Surface {
                operation: "NvBufSurfaceMap",
                status,
            });
        }
        Ok(())
    }

    fn params(&self) -> &sys::SurfaceParams {
        // SAFETY: the batch holds one buffer, alive as long as `self`.
        unsafe { &*(*self.surface).surface_list }
    }

    /// The buffer's DMABUF fd, by which libnvjpeg finds it.
    pub fn fd(&self) -> c_int {
        self.fd
    }

    /// Copies `frame` into the buffer and hands it to the hardware.
    pub fn write(&mut self, frame: Nv12<'_>) -> Result<()> {
        debug_assert!(self.mapped, "only a pitch-linear surface can be written");
        let width = self.width as usize;
        let height = self.height as usize;
        if frame.stride < width {
            return Err(Error::StrideTooSmall {
                stride: frame.stride,
                width: self.width,
            });
        }
        let needed = frame.stride * (height + height / 2);
        if frame.data.len() < needed {
            return Err(Error::FrameTooShort {
                len: frame.data.len(),
                needed,
                stride: frame.stride,
            });
        }
        let params = self.params();
        let planes = [(0, height), (frame.stride * height, height / 2)];
        for (plane, (source_offset, rows)) in planes.into_iter().enumerate() {
            let pitch = params.plane_params.pitch[plane] as usize;
            let destination = params.mapped_addr.addr[plane].cast::<u8>();
            for row in 0..rows {
                let source = &frame.data[source_offset + row * frame.stride..][..width];
                // SAFETY: the mapped plane holds `rows` rows of `pitch >= width`
                // bytes, which `map` checked.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        source.as_ptr(),
                        destination.add(row * pitch),
                        width,
                    );
                }
            }
        }
        // SAFETY: `surface` is a live, mapped batch; plane -1 syncs every plane.
        let status = unsafe { (self.api.surface_sync_for_device)(self.surface, 0, -1) };
        if status != 0 {
            return Err(Error::Surface {
                operation: "NvBufSurfaceSyncForDevice",
                status,
            });
        }
        Ok(())
    }

    /// Converts this buffer into `destination` (of the same size) on the VIC.
    pub fn transform_into(&self, destination: &Nv12Surface) -> Result<()> {
        let mut rect = sys::TransformRect {
            top: 0,
            left: 0,
            width: self.width,
            height: self.height,
        };
        let mut destination_rect = sys::TransformRect { ..rect };
        let mut params = sys::TransformParams {
            transform_flag: sys::NVBUFSURF_TRANSFORM_FILTER,
            transform_flip: 0,
            transform_filter: 0,
            src_rect: &raw mut rect,
            dst_rect: &raw mut destination_rect,
        };
        // SAFETY: both surfaces are live batches; `params` outlives the call.
        let status = unsafe {
            (self.api.surface_transform)(self.surface, destination.surface, &raw mut params)
        };
        if status != 0 {
            return Err(Error::Surface {
                operation: "NvBufSurfTransform",
                status,
            });
        }
        Ok(())
    }
}

impl Drop for Nv12Surface {
    fn drop(&mut self) {
        // SAFETY: `surface` came from NvBufSurfaceAllocate and is released once.
        unsafe {
            if self.mapped {
                (self.api.surface_unmap)(self.surface, 0, -1);
            }
            (self.api.surface_destroy)(self.surface);
        }
    }
}
