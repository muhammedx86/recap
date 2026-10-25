//! A small, low-rate copy of the picture for live detection.
//!
//! An OBS view renders the same scene into its own mix at the analysis height (the GPU does the
//! scaling, so only that small picture is read back). A tiny raw-video output of Recap's own
//! (`recap_live_detect`, registered once) is attached to that mix the way OBS's virtual camera
//! is: starting it is what makes OBS download the mix's frames. Its callback runs on OBS's
//! video thread, keeps a frame only as often as the detectors sample, copies the luma plane and
//! hands it to a background-priority worker, dropping it if the worker is still busy. OBS and
//! the game never wait for detection.

use std::{
    ffi::{c_char, c_void},
    sync::{mpsc, Arc, Mutex, Once},
    thread::JoinHandle,
};

use image::GrayImage;
use libobs_wrapper::{
    data::video::ObsVideoInfoBuilder,
    enums::{ObsScaleType, ObsVideoRange},
    run_with_obs,
    runtime::ObsRuntime,
    sys,
    unsafe_send::Sendable,
};
use recap_core::live::LiveScanner;

/// Where frames go (one live feed at a time).
struct Target {
    tx: mpsc::SyncSender<(u64, GrayImage)>,
    w: u32,
    h: u32,
    period_ms: u64,
    last_ms: u64,
}

static TARGET: Mutex<Option<Target>> = Mutex::new(None);
static REGISTER: Once = Once::new();
const OUTPUT_ID: &[u8] = b"recap_live_detect\0";

unsafe extern "C" fn out_name(_: *mut c_void) -> *const c_char {
    c"Recap live detection".as_ptr()
}

unsafe extern "C" fn out_create(_settings: *mut sys::obs_data_t, output: *mut sys::obs_output_t) -> *mut c_void {
    Box::into_raw(Box::new(output)) as *mut c_void
}

unsafe extern "C" fn out_destroy(data: *mut c_void) {
    if !data.is_null() {
        // Safety: created by out_create.
        drop(unsafe { Box::from_raw(data as *mut *mut sys::obs_output_t) });
    }
}

unsafe extern "C" fn out_start(data: *mut c_void) -> bool {
    // Safety: data is the box from out_create; the output outlives its data.
    unsafe {
        let output = *(data as *mut *mut sys::obs_output_t);
        sys::obs_output_can_begin_data_capture(output, 0) && sys::obs_output_begin_data_capture(output, 0)
    }
}

unsafe extern "C" fn out_stop(data: *mut c_void, _ts: u64) {
    // Safety: as in out_start.
    unsafe {
        let output = *(data as *mut *mut sys::obs_output_t);
        sys::obs_output_end_data_capture(output);
    }
}

/// Keep the luma plane (the first plane of NV12) when a sample is due. Runs on OBS's video
/// thread: no waiting, never unwinds into C.
unsafe extern "C" fn out_raw_video(_data: *mut c_void, frame: *mut sys::video_data) {
    let _ = std::panic::catch_unwind(|| {
        let Ok(mut guard) = TARGET.try_lock() else { return };
        let Some(t) = guard.as_mut() else { return };
        let now = crate::session::unix_ms();
        if now < t.last_ms + t.period_ms * 9 / 10 {
            return;
        }
        // Safety: OBS passes a frame valid for the duration of the call.
        let f = unsafe { &*frame };
        let (w, h) = (t.w as usize, t.h as usize);
        let stride = f.linesize[0] as usize;
        if f.data[0].is_null() || stride < w {
            return;
        }
        let mut buf = vec![0u8; w * h];
        for y in 0..h {
            // Safety: the plane holds `h` rows of `stride` bytes.
            unsafe { std::ptr::copy_nonoverlapping(f.data[0].add(y * stride), buf.as_mut_ptr().add(y * w), w) };
        }
        if let Some(img) = GrayImage::from_raw(t.w, t.h, buf) {
            if t.tx.try_send((now, img)).is_ok() {
                t.last_ms = now;
            }
        }
    });
}

/// Register the output type with OBS (once per process; on the OBS thread).
fn register() {
    REGISTER.call_once(|| {
        // Safety: an all-zero obs_output_info is valid (null pointers and no callbacks); OBS
        // copies the struct.
        let mut info: sys::obs_output_info = unsafe { std::mem::zeroed() };
        info.id = OUTPUT_ID.as_ptr() as *const c_char;
        info.flags = sys::OBS_OUTPUT_VIDEO;
        info.get_name = Some(out_name);
        info.create = Some(out_create);
        info.destroy = Some(out_destroy);
        info.start = Some(out_start);
        info.stop = Some(out_stop);
        info.raw_video = Some(out_raw_video);
        // Safety: on the OBS thread, with a complete struct.
        unsafe { sys::obs_register_output_s(&info, std::mem::size_of::<sys::obs_output_info>()) };
    });
}

pub struct LiveFeed {
    runtime: ObsRuntime,
    view: Sendable<*mut sys::obs_view_t>,
    output: Sendable<*mut sys::obs_output_t>,
    worker: Option<JoinHandle<()>>,
    pub size: (u32, u32),
}

impl LiveFeed {
    /// Start feeding `scanner` from `scene_source`, whose canvas is `canvas` at `fps`.
    pub fn start(
        runtime: ObsRuntime,
        scene_source: Sendable<*mut sys::obs_source_t>,
        canvas: (u32, u32),
        fps: u32,
        analysis_height: u32,
        sample_fps: f32,
        scanner: Arc<Mutex<LiveScanner>>,
    ) -> anyhow::Result<Self> {
        let h = (analysis_height.min(canvas.1).max(2)) & !1;
        let w = (((canvas.0 as u64 * h as u64 / canvas.1.max(1) as u64) as u32).max(2)) & !1;
        let period_ms = (1000.0 / sample_fps.max(1.0)) as u64;
        let ovi = ObsVideoInfoBuilder::new()
            .fps_num(fps)
            .fps_den(1)
            .base_width(canvas.0)
            .base_height(canvas.1)
            .output_width(w)
            .output_height(h)
            .range(ObsVideoRange::Full)
            .scale_type(ObsScaleType::Bilinear)
            .build();
        // One frame in flight at most: a busy worker means frames are skipped, not queued.
        let (tx, rx) = mpsc::sync_channel::<(u64, GrayImage)>(1);
        *TARGET.lock().unwrap_or_else(|e| e.into_inner()) = Some(Target {
            tx,
            w,
            h,
            period_ms,
            last_ms: 0,
        });
        let ovi_ptr = Sendable(ovi.as_ptr());
        let src = scene_source;
        let made = run_with_obs!(runtime, (), move || {
            let (ovi_ptr, src) = (ovi_ptr, src);
            register();
            // Safety: on the OBS thread; the scene source is alive (the backend owns the scene)
            // and `ovi` outlives this call.
            unsafe {
                let view = sys::obs_view_create();
                if view.is_null() {
                    return Err("could not create a view");
                }
                sys::obs_view_set_source(view, 0, src.0);
                let video = sys::obs_view_add2(view, ovi_ptr.0);
                if video.is_null() {
                    sys::obs_view_set_source(view, 0, std::ptr::null_mut());
                    sys::obs_view_destroy(view);
                    return Err("could not add the view's video mix");
                }
                let output = sys::obs_output_create(
                    OUTPUT_ID.as_ptr() as *const c_char,
                    c"recap_live".as_ptr(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                if output.is_null() {
                    sys::obs_view_remove(view);
                    sys::obs_view_set_source(view, 0, std::ptr::null_mut());
                    sys::obs_view_destroy(view);
                    return Err("could not create the live output");
                }
                sys::obs_output_set_media(output, video, sys::obs_get_audio());
                if !sys::obs_output_start(output) {
                    sys::obs_output_release(output);
                    sys::obs_view_remove(view);
                    sys::obs_view_set_source(view, 0, std::ptr::null_mut());
                    sys::obs_view_destroy(view);
                    return Err("the live output did not start");
                }
                Ok((Sendable(view), Sendable(output)))
            }
        });
        drop(ovi);
        let (view, output) = match made {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                *TARGET.lock().unwrap_or_else(|e| e.into_inner()) = None;
                anyhow::bail!("{e} ({w}x{h})");
            }
            Err(e) => {
                *TARGET.lock().unwrap_or_else(|e| e.into_inner()) = None;
                anyhow::bail!("{e:?}");
            }
        };
        let worker = std::thread::Builder::new()
            .name("live-detect".into())
            .spawn(move || {
                recap_core::background::lower_current_thread();
                while let Ok((t, img)) = rx.recv() {
                    if let Ok(mut s) = scanner.lock() {
                        s.push(t, &img);
                    }
                }
            })
            .ok();
        eprintln!("live detection: {w}x{h}, a frame every {period_ms} ms");
        Ok(Self {
            runtime,
            view,
            output,
            worker,
            size: (w, h),
        })
    }
}

impl Drop for LiveFeed {
    fn drop(&mut self) {
        let (view, output) = (Sendable(self.view.0), Sendable(self.output.0));
        let runtime = self.runtime.clone();
        let _ = run_with_obs!(runtime, (), move || {
            let (view, output) = (view, output);
            // Safety: on the OBS thread; the output and view were created by `start`.
            unsafe {
                sys::obs_output_stop(output.0);
                sys::obs_output_release(output.0);
                sys::obs_view_remove(view.0);
                sys::obs_view_set_source(view.0, 0, std::ptr::null_mut());
                sys::obs_view_destroy(view.0);
            }
        });
        // Dropping the sender ends the worker.
        *TARGET.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}
