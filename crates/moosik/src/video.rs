//! In-app music video, on every platform.
//!
//! mpv's render API has only an OpenGL backend, which would tie this view to whichever GPU
//! renderer egui happens to be using and to mpv sharing a surface with it. So this drives mpv's
//! **software** render context instead: mpv writes each frame into a CPU buffer that is uploaded
//! as an egui texture. One code path on every OS and under any renderer, at the cost of a copy
//! per frame; the view is opt-in and the resolution is capped.
//!
//! `libmpv2`'s safe wrapper only exposes the OpenGL params, so this uses `libmpv2-sys` directly.

use std::ffi::c_void;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use libmpv2::Mpv;
use libmpv2_sys as sys;
use sys::{
    mpv_render_param_type_MPV_RENDER_PARAM_SW_FORMAT as SW_FORMAT,
    mpv_render_param_type_MPV_RENDER_PARAM_SW_POINTER as SW_POINTER,
    mpv_render_param_type_MPV_RENDER_PARAM_SW_SIZE as SW_SIZE,
    mpv_render_param_type_MPV_RENDER_PARAM_SW_STRIDE as SW_STRIDE,
};

/// Bytes per pixel `rgb0` writes.
const BPP: usize = 4;

/// mpv's software renderer wants both the stride and the pixel pointer to be multiples of 64
/// bytes for its fast SIMD paths; below that it can fall back to copying the entire frame. The
/// surface width is padded to 16 pixels, which makes the stride a multiple of 64, and the buffer
/// itself is allocated 64-byte aligned.
const ALIGN: usize = 64;
const PAD_PIXELS: usize = ALIGN / BPP;

/// A heap buffer whose pointer is 64-byte aligned. `Vec<u8>` guarantees only 1.
struct Aligned {
    ptr: std::ptr::NonNull<u8>,
    layout: std::alloc::Layout,
}

impl Aligned {
    fn new(len: usize) -> Self {
        let layout =
            std::alloc::Layout::from_size_align(len.max(1), ALIGN).expect("valid buffer layout");
        // SAFETY: the layout has non-zero size.
        let ptr = unsafe { std::alloc::alloc(layout) };
        Self {
            ptr: std::ptr::NonNull::new(ptr).expect("video buffer allocation failed"),
            layout,
        }
    }

    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr.as_ptr()
    }

    fn as_slice(&self) -> &[u8] {
        // SAFETY: the allocation is live for `self.layout.size()` bytes.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.layout.size()) }
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: as above, and `&mut self` makes this the only reference.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.layout.size()) }
    }
}

impl Drop for Aligned {
    fn drop(&mut self) {
        // SAFETY: the pointer and layout came from the same `alloc` call.
        unsafe { std::alloc::dealloc(self.ptr.as_ptr(), self.layout) }
    }
}

/// The callback mpv fires from its own thread when a frame is ready.
struct Callback {
    dirty: Arc<AtomicBool>,
    ctx: egui::Context,
}

unsafe extern "C" fn on_update(data: *mut c_void) {
    // SAFETY: `data` is the `Callback` leaked in `Video::new`, alive until `Drop`.
    let callback = unsafe { &*(data as *const Callback) };
    callback.dirty.store(true, Ordering::SeqCst);
    callback.ctx.request_repaint();
}

pub struct Video {
    ctx: *mut sys::mpv_render_context,
    callback: *mut Callback,
    dirty: Arc<AtomicBool>,
    pixels: Aligned,
    /// The surface size last rendered, in padded pixels.
    size: (usize, usize),
    texture: Option<egui::TextureHandle>,
}

impl Video {
    /// Create the software render context for `mpv`, or `None` if mpv refuses.
    pub fn new(mpv: &Mpv, ctx: egui::Context) -> Option<Self> {
        let dirty = Arc::new(AtomicBool::new(false));
        let callback = Box::into_raw(Box::new(Callback {
            dirty: Arc::clone(&dirty),
            ctx,
        }));
        let api = sys::MPV_RENDER_API_TYPE_SW.as_ptr() as *mut c_void;
        let mut params = [
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
                data: api,
            },
            sys::mpv_render_param {
                type_: 0,
                data: ptr::null_mut(),
            },
        ];
        let mut render: *mut sys::mpv_render_context = ptr::null_mut();
        let err = unsafe {
            sys::mpv_render_context_create(&mut render, mpv.ctx.as_ptr(), params.as_mut_ptr())
        };
        if err < 0 || render.is_null() {
            // SAFETY: the box was never handed to mpv.
            unsafe { drop(Box::from_raw(callback)) };
            return None;
        }
        unsafe {
            sys::mpv_render_context_set_update_callback(
                render,
                Some(on_update),
                callback as *mut c_void,
            );
        }
        Some(Self {
            ctx: render,
            callback,
            dirty,
            pixels: Aligned::new(0),
            size: (0, 0),
            texture: None,
        })
    }

    /// Draw a frame into `size` physical pixels and return the texture. When nothing has changed
    /// since the last call, the existing texture is returned untouched — the update callback
    /// wakes the UI only when mpv has a new frame.
    pub fn frame(&mut self, ui: &egui::Ui, size: (usize, usize)) -> Option<egui::TextureHandle> {
        let (w, h) = size;
        if w == 0 || h == 0 {
            return self.texture.clone();
        }
        // Pad the surface width so the stride is 64-byte aligned; mpv scales the video into
        // the slightly wider surface, which is invisible (a few pixels at most).
        let w = w.next_multiple_of(PAD_PIXELS);
        let update = unsafe { sys::mpv_render_context_update(self.ctx) };
        let ready = update & u64::from(sys::mpv_render_update_flag_MPV_RENDER_UPDATE_FRAME) != 0;
        let resized = self.size != (w, h);
        let asked = self.dirty.swap(false, Ordering::SeqCst);
        if !ready && !resized && !asked {
            return self.texture.clone();
        }

        let stride = w * BPP;
        let len = stride * h;
        if self.pixels.as_slice().len() != len {
            self.pixels = Aligned::new(len);
        }
        self.size = (w, h);
        let mut surface = [w as i32, h as i32];
        let mut stride_value = stride;
        let mut params = [
            sys::mpv_render_param {
                type_: SW_SIZE,
                data: surface.as_mut_ptr() as *mut c_void,
            },
            sys::mpv_render_param {
                type_: SW_FORMAT,
                data: c"rgb0".as_ptr() as *mut c_void,
            },
            sys::mpv_render_param {
                type_: SW_STRIDE,
                data: &mut stride_value as *mut usize as *mut c_void,
            },
            sys::mpv_render_param {
                type_: SW_POINTER,
                data: self.pixels.as_mut_ptr() as *mut c_void,
            },
            sys::mpv_render_param {
                type_: 0,
                data: ptr::null_mut(),
            },
        ];
        let err = unsafe { sys::mpv_render_context_render(self.ctx, params.as_mut_ptr()) };
        if err < 0 {
            return self.texture.clone();
        }

        // `rgb0` leaves the fourth byte unused; egui wants an opaque alpha there. With alpha at
        // 255, premultiplied and straight are the same, so egui's conversion pass is skipped.
        for alpha in self
            .pixels
            .as_mut_slice()
            .iter_mut()
            .skip(BPP - 1)
            .step_by(BPP)
        {
            *alpha = 255;
        }
        let image = egui::ColorImage::from_rgba_premultiplied([w, h], self.pixels.as_slice());
        match &mut self.texture {
            Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
            None => {
                self.texture = Some(ui.ctx().load_texture(
                    "moosik-video",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        self.texture.clone()
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        unsafe {
            sys::mpv_render_context_free(self.ctx);
            drop(Box::from_raw(self.callback));
        }
    }
}
