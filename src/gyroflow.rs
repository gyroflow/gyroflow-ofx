use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;

use gyroflow_core::{ StabilizationManager, stabilization::{ RGBA8, RGBA16, RGBAf } };
use gyroflow_core::gpu::{ BufferDescription, Buffers, BufferSource };
use lru::LruCache;
use measure_time::*;
use ofx::*;
use parking_lot::Mutex;
use super::fuscript::*;

plugin_module!(
    "nl.smslv.gyroflowofx.fisheyestab_v1",
    ApiVersion(1),
    PluginVersion(1, 2),
    GyroflowPlugin::default
);

#[derive(Default)]
struct GyroflowPlugin { }

#[allow(unused)]
struct InstanceData {
    source_clip: ClipInstance,
    output_clip: ClipInstance,

    param_project_data: ParamHandle<String>,
    param_project_path: ParamHandle<String>,
    param_fov: ParamHandle<Double>,
    param_smoothness: ParamHandle<Double>,
    param_lens_correction_strength: ParamHandle<Double>,
    param_disable_stretch: ParamHandle<Bool>,
    param_disable_rotation: ParamHandle<Bool>,
    param_status: ParamHandle<Bool>,
    param_open_in_gyroflow: ParamHandle<Bool>,
    param_toggle_overview: ParamHandle<Bool>,
    param_reload_project: ParamHandle<Bool>,
    param_dont_draw_outside: ParamHandle<Bool>,
    gyrodata: LruCache<String, Arc<StabilizationManager>>,

    original_video_size: (usize, usize),
    original_output_size: (usize, usize),

    current_file_info_pending: Arc<AtomicBool>,
    current_file_info: Arc<Mutex<Option<CurrentFileInfo>>>
}

impl InstanceData {
    fn update_loaded_state(&mut self, loaded: bool) {
        let _ = self.param_fov.set_enabled(loaded);
        let _ = self.param_smoothness.set_enabled(loaded);
        let _ = self.param_lens_correction_strength.set_enabled(loaded);
        let _ = self.param_disable_rotation.set_enabled(loaded);
        let _ = self.param_disable_stretch.set_enabled(loaded);
        let _ = self.param_toggle_overview.set_enabled(loaded);
        let _ = self.param_reload_project.set_enabled(loaded);
        let _ = self.param_status.set_label(if loaded { "OK" } else { "Project not loaded" });
        let _ = self.param_open_in_gyroflow.set_label(if loaded { "Open in Gyroflow" } else { "Open Gyroflow" });
    }

    fn gyrodata(&mut self, bit_depth: BitDepth, output_rect: RectI, device: i32) -> Result<Arc<StabilizationManager>> {
        let disable_stretch = self.param_disable_stretch.get_value()?;
        let disable_rotation = self.param_disable_rotation.get_value()?;

        let source_rect = self.source_clip.get_region_of_definition(0.0)?;
        let in_size = ((source_rect.x2 - source_rect.x1) as usize, (source_rect.y2 - source_rect.y1) as usize);
        let out_size = ((output_rect.x2 - output_rect.x1) as usize, (output_rect.y2 - output_rect.y1) as usize);

        let path = self.param_project_path.get_value()?;
        if path.is_empty() {
            self.update_loaded_state(false);
            return Err(Error::UnknownError);
        }
        let key = format!("{path}{bit_depth:?}{in_size:?}{out_size:?}{disable_stretch}{disable_rotation}{device}");
        let stab = if let Some(stab) = self.gyrodata.get(&key) {
            stab.clone()
        } else {
            let stab = StabilizationManager::default();

            if !path.ends_with(".gyroflow") {
                // Try to load from video file
                if let Err(e) = stab.load_video_file(&path, None) {
                    log::error!("An error occured: {e:?}");
                    self.update_loaded_state(false);
                    return Err(Error::UnknownError);
                }
            } else {
                let project_data = {
                    if let Ok(data) = std::fs::read_to_string(&path) {
                        self.param_project_data.set_value(data.clone())?;
                        data
                    } else {
                        self.param_project_data.get_value()?
                    }
                };
                let mut is_preset = false;
                stab.import_gyroflow_data(project_data.as_bytes(), true, Some(std::path::PathBuf::from(path)), |_|(), Arc::new(AtomicBool::new(false)), &mut is_preset).map_err(|e| {
                    error!("load_gyro_data error: {}", &e);
                    self.update_loaded_state(false);
                    Error::UnknownError
                })?;
            }

            let loaded = {
                let params = stab.params.read();
                self.original_video_size = params.video_size;
                self.original_output_size = params.video_output_size;
                let loaded = params.duration_ms > 0.0;
                if loaded {
                    let smoothness = stab.smoothing.read().current().get_parameter("smoothness");

                    self.param_fov.set_value(params.fov)?;
                    self.param_smoothness.set_value(smoothness)?;
                    self.param_lens_correction_strength.set_value((params.lens_correction_amount * 100.0).min(100.0))?;
                }
                loaded
            };

            self.update_loaded_state(loaded);

            if disable_stretch {
                let (x_stretch, y_stretch) = {
                    let lens = stab.lens.read();
                    (lens.input_horizontal_stretch, lens.input_vertical_stretch)
                };
                if (x_stretch > 0.01 && x_stretch != 1.0) || (y_stretch > 0.01 && y_stretch != 1.0) {
                    {
                        let mut params = stab.params.write();
                        params.video_size.0 = (params.video_size.0 as f64 * x_stretch).round() as usize;
                        params.video_size.1 = (params.video_size.1 as f64 * y_stretch).round() as usize;
                    }
                    {
                        let mut lens = stab.lens.write();
                        lens.input_horizontal_stretch = 1.0;
                        lens.input_vertical_stretch = 1.0;
                    }
                }
            }
            if disable_rotation {
                stab.params.write().video_rotation = 0.0;
            }

            stab.set_fov_overview(self.param_toggle_overview.get_value()?);

            let video_size = {
                let mut params = stab.params.write();
                params.framebuffer_inverted = true;
                params.video_size
            };

            let org_ratio = video_size.0 as f64 / video_size.1 as f64;

            let src_rect = Self::get_center_rect(in_size.0, in_size.1, org_ratio);
            stab.set_size(src_rect.2, src_rect.3);
            stab.set_output_size(out_size.0, out_size.1);

            {
                let mut stab = stab.stabilization.write();
                stab.interpolation = gyroflow_core::stabilization::Interpolation::Lanczos4;
            }

            stab.invalidate_smoothing();
            stab.recompute_blocking();
            stab.params.write().calculate_ramped_timestamps(&stab.keyframes.read());

            self.gyrodata
                .put(key.to_owned(), Arc::new(stab));
            self.gyrodata
                .get(&key)
                .map(Arc::clone)
                .ok_or(Error::UnknownError)?
        };

        Ok(stab)
    }

    pub fn check_pending_file_info(&mut self) -> Result<()> {
        if self.current_file_info_pending.load(SeqCst) {
            self.current_file_info_pending.store(false, SeqCst);
            let lock = self.current_file_info.lock();
            if let Some(ref current_file) = *lock {
                if let Some(proj) = &current_file.project_path {
                    self.param_project_path.set_value(proj.to_string())?;
                } else {
                    // Try to use the video directly
                    self.param_project_path.set_value(current_file.file_path.clone())?;
                }
            }
        }
        Ok(())
    }

    fn get_center_rect(width: usize, height: usize, org_ratio: f64) -> (usize, usize, usize, usize) {
        // If aspect ratio is different
        let new_ratio = width as f64 / height as f64;
        if (new_ratio - org_ratio).abs() > 0.1 {
            // Get center rect of original aspect ratio
            let rect = if new_ratio > org_ratio {
                ((height as f64 * org_ratio).round() as usize, height)
            } else {
                (width, (width as f64 / org_ratio).round() as usize)
            };
            (
                (width - rect.0) / 2, // x
                (height - rect.1) / 2, // y
                rect.0, // width
                rect.1 // height
            )
        } else {
            (0, 0, width, height)
        }
    }
}

struct PerFrameParams { }

impl Execute for GyroflowPlugin {
    #[allow(clippy::float_cmp)]
    fn execute(&mut self, _plugin_context: &PluginContext, action: &mut Action) -> Result<Int> {
        use Action::*;

        // log::debug!("action: {action:?}");

        match *action {
            Render(ref mut effect, ref in_args) => {
                let _time = std::time::Instant::now();

                let mut device = -1;
                #[cfg(any(target_os = "windows", target_os = "linux"))]
                if in_args.get_cuda_enabled().unwrap_or_default() {
                    device = gyroflow_core::gpu::wgpu_interop_cuda::get_current_cuda_device();
                }

                let time = in_args.get_time()?;
                let instance_data: &mut InstanceData = effect.get_instance_data()?;

                instance_data.check_pending_file_info()?;

                let output_image = instance_data.output_clip.get_image_mut(time)?;
                let output_image = output_image.borrow_mut();

                let fov = instance_data.param_fov.get_value_at_time(time)?;
                let lens_correction_strength = instance_data.param_lens_correction_strength.get_value_at_time(time)? / 100.0;
                let smoothness = instance_data.param_smoothness.get_value_at_time(time)?;

                let output_rect: RectI = output_image.get_region_of_definition()?;

                let stab = instance_data.gyrodata(output_image.get_pixel_depth()?, output_rect, device)?;

                let params = stab.params.read();
                let params_fov = params.fov;
                let params_lens_correction_strength = params.lens_correction_amount;
                let params_smoothness = stab.smoothing.read().current().get_parameter("smoothness");
                let fps = params.fps;
                let src_fps = instance_data.source_clip.get_frame_rate().unwrap_or(fps);
                let org_ratio = params.video_size.0 as f64 / params.video_size.1 as f64;
                let (has_quats, has_offsets) = {
                    let gyro = stab.gyro.read();
                    (!gyro.org_quaternions.is_empty(), !gyro.get_offsets().is_empty())
                };

                let frame_number = (params.frame_count - 1) as f64;

                let mut speed_stretch = 1.0;
                if let Ok(range) = instance_data.source_clip.get_frame_range() {
                    if range.max > 0.0 {
                        if (frame_number - range.max).abs() > 2.0 {
                            speed_stretch = frame_number / range.max;
                        }
                    }
                }
                if (src_fps - fps).abs() > 0.01 {
                    instance_data.param_status.set_label("Timeline fps mismatch!")?;
                    instance_data.param_status.set_hint("Timeline frame rate doesn't match the clip frame rate!")?;
                    if instance_data.param_status.get_value()? {
                        instance_data.param_status.set_value(false)?;
                    }
                } else if !has_quats && !has_offsets {
                    instance_data.param_status.set_label("Not synced. Open in Gyroflow")?;
                    instance_data.param_status.set_hint("Gyro data is not synced with the video, open the video in Gyroflow and add sync points (eg. by doing autosync)")?;
                    if instance_data.param_status.get_value()? {
                        instance_data.param_status.set_value(false)?;
                    }
                } else {
                    instance_data.param_status.set_label("OK")?;
                    instance_data.param_status.set_hint("OK")?;
                    if !instance_data.param_status.get_value()? {
                        instance_data.param_status.set_value(true)?;
                    }
                }

                speed_stretch *= src_fps / fps;

                let mut time = time;
                let mut timestamp_us = ((time / src_fps * 1_000_000.0) * speed_stretch).round() as i64;

                let source_timestamp_us = params.get_source_timestamp_at_ramped_timestamp(timestamp_us);
                drop(params);

                if source_timestamp_us != timestamp_us {
                    time = (source_timestamp_us as f64 / speed_stretch / 1_000_000.0 * src_fps).round();
                    timestamp_us = ((time / src_fps * 1_000_000.0) * speed_stretch).round() as i64;
                }

                let source_image = instance_data.source_clip.get_image(time)?;

                let source_rect: RectI = source_image.get_region_of_definition()?;

                let src_stride = source_image.get_row_bytes()? as usize;
                let out_stride = output_image.get_row_bytes()? as usize;
                let src_size = ((source_rect.x2 - source_rect.x1) as usize, (source_rect.y2 - source_rect.y1) as usize, src_stride);
                let out_size = ((output_rect.x2 - output_rect.x1) as usize, (output_rect.y2 - output_rect.y1) as usize, out_stride);

                let src_rect = InstanceData::get_center_rect(src_size.0, src_size.1, org_ratio);

                let mut out_rect = if instance_data.param_dont_draw_outside.get_value_at_time(time)? {
                    let output_ratio = out_size.0 as f64 / out_size.1 as f64;
                    let mut rect = InstanceData::get_center_rect(src_rect.2, src_rect.3, output_ratio);
                    rect.0 += src_rect.0;
                    rect.1 += src_rect.1;
                    Some(rect)
                } else {
                    None
                };
                let out_scale = output_image.get_render_scale()?;
                if out_scale.x != 1.0 || out_scale.y != 1.0 {
                    // log::debug!("out_scale: {:?}", out_scale);
                    let w = (out_size.0 as f64 * out_scale.x as f64).round() as usize;
                    let h = (out_size.1 as f64 * out_scale.y as f64).round() as usize;
                    out_rect = Some((
                        0,
                        out_size.1 - h, // because the coordinates are inverted
                        w,
                        h
                    ));
                }

                // log::debug!("src_size: {src_size:?} | src_rect: {src_rect:?}");
                // log::debug!("out_size: {out_size:?} | out_rect: {out_rect:?}");

                if (params_fov - fov).abs() > 0.001 {
                    stab.params.write().fov = fov;
                    stab.recompute_undistortion();
                }
                if (params_lens_correction_strength - lens_correction_strength).abs() > 0.001 {
                    stab.params.write().lens_correction_amount = lens_correction_strength;
                    stab.recompute_adaptive_zoom();
                    stab.recompute_undistortion();
                }
                if (params_smoothness - smoothness).abs() > 0.001 {
                    stab.smoothing.write().current_mut().set_parameter("smoothness", smoothness);
                    stab.recompute_blocking();
                }

                let mut buffers =
                    if in_args.get_opencl_enabled().unwrap_or_default() {
                        use std::ffi::c_void;
                        let queue = in_args.get_opencl_command_queue()? as *mut c_void;
                        Some(Buffers {
                            input: BufferDescription {
                                size: src_size,
                                rect: Some(src_rect),
                                data: BufferSource::OpenCL { texture: source_image.get_data()? as *mut c_void, queue },
                                texture_copy: false
                            },
                            output: BufferDescription {
                                size: out_size,
                                rect: out_rect,
                                data: BufferSource::OpenCL { texture: output_image.get_data()? as *mut c_void, queue },
                                texture_copy: false
                            }
                        })
                    } else if in_args.get_metal_enabled().unwrap_or_default() {
                        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
                        { None }
                        #[cfg(any(target_os = "macos", target_os = "ios"))]
                        {
                            let in_ptr  = source_image.get_data()? as *mut metal::MTLBuffer;
                            let out_ptr = output_image.get_data()? as *mut metal::MTLBuffer;
                            let command_queue = in_args.get_metal_command_queue()? as *mut metal::MTLCommandQueue;

                            Some(Buffers {
                                input: BufferDescription {
                                    size: src_size,
                                    rect: Some(src_rect),
                                    data: BufferSource::MetalBuffer { buffer: in_ptr, command_queue },
                                    texture_copy: false
                                },
                                output: BufferDescription {
                                    size: out_size,
                                    rect: out_rect,
                                    data: BufferSource::MetalBuffer { buffer: out_ptr, command_queue },
                                    texture_copy: false
                                }
                            })
                        }
                    } else if in_args.get_cuda_enabled().unwrap_or_default() {
                        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
                        { None }
                        #[cfg(any(target_os = "windows", target_os = "linux"))]
                        {
                            let in_ptr  = source_image.get_data()? as *mut std::ffi::c_void;
                            let out_ptr = output_image.get_data()? as *mut std::ffi::c_void;

                            Some(Buffers {
                                input: BufferDescription {
                                    size: src_size,
                                    rect: Some(src_rect),
                                    data: BufferSource::CUDABuffer { buffer: in_ptr },
                                    texture_copy: true
                                },
                                output: BufferDescription {
                                    size: out_size,
                                    rect: out_rect,
                                    data: BufferSource::CUDABuffer { buffer: out_ptr },
                                    texture_copy: true
                                }
                            })
                        }
                    } else {
                        use std::slice::from_raw_parts_mut;
                        let src_buf = unsafe { match source_image.get_pixel_depth()? {
                            BitDepth::Byte  => { let b = source_image.get_descriptor::<RGBAColourB>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) },
                            BitDepth::Short => { let b = source_image.get_descriptor::<RGBAColourS>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) },
                            BitDepth::Float => { let b = source_image.get_descriptor::<RGBAColourF>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) }
                        } };
                        let dst_buf = unsafe { match output_image.get_pixel_depth()? {
                            BitDepth::Byte  => { let b = output_image.get_descriptor::<RGBAColourB>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) },
                            BitDepth::Short => { let b = output_image.get_descriptor::<RGBAColourS>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) },
                            BitDepth::Float => { let b = output_image.get_descriptor::<RGBAColourF>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) }
                        } };

                        Some(Buffers {
                            input: BufferDescription {
                                size: src_size,
                                rect: Some(src_rect),
                                data: BufferSource::Cpu { buffer: src_buf },
                                texture_copy: false
                            },
                            output: BufferDescription {
                                size: out_size,
                                rect: out_rect,
                                data: BufferSource::Cpu { buffer: dst_buf },
                                texture_copy: false
                            }
                        })
                    };

                let processed = if let Some(ref mut buffers) = buffers {
                    match output_image.get_pixel_depth()? {
                        BitDepth::Byte  => stab.process_pixels::<RGBA8> (timestamp_us, buffers),
                        BitDepth::Short => stab.process_pixels::<RGBA16>(timestamp_us, buffers),
                        BitDepth::Float => stab.process_pixels::<RGBAf> (timestamp_us, buffers)
                    }
                } else {
                    None
                };

                // log::info!("Rendered | {}x{} in {:.2}ms: {:?}", src_size.0, src_size.1, _time.elapsed().as_micros() as f64 / 1000.0, processed);

                if effect.abort()? || !processed.is_some() {
                    FAILED
                } else {
                    OK
                }
            }

            CreateInstance(ref mut effect) => {
                let param_set = effect.parameter_set()?;
                // let mut effect_props: EffectInstance = effect.properties()?;

                let source_clip = effect.get_simple_input_clip()?;
                let output_clip = effect.get_output_clip()?;

                effect.set_instance_data(InstanceData {
                    source_clip,
                    output_clip,
                    param_project_data:             param_set.parameter("ProjectData")?,
                    param_project_path:             param_set.parameter("gyrodata")?,
                    param_smoothness:               param_set.parameter("Smoothness")?,
                    param_lens_correction_strength: param_set.parameter("LensCorrectionStrength")?,
                    param_fov:                      param_set.parameter("FOV")?,
                    param_disable_stretch:          param_set.parameter("DisableStretch")?,
                    param_disable_rotation:         param_set.parameter("DisableRotation")?,
                    param_status:                   param_set.parameter("Status")?,
                    param_open_in_gyroflow:         param_set.parameter("OpenGyroflow")?,
                    param_reload_project:           param_set.parameter("ReloadProject")?,
                    param_toggle_overview:          param_set.parameter("ToggleOverview")?,
                    param_dont_draw_outside:        param_set.parameter("DontDrawOutside")?,
                    gyrodata:                       LruCache::new(std::num::NonZeroUsize::new(8).unwrap()),
                    original_output_size:           (0, 0),
                    original_video_size:            (0, 0),
                    current_file_info:              Arc::new(Mutex::new(None)),
                    current_file_info_pending:      Arc::new(AtomicBool::new(false))
                })?;

                OK
            }
            InstanceChanged(ref mut effect, ref mut in_args) => {
                if in_args.get_name()? == "Browse" {
                    let instance_data: &mut InstanceData = effect.get_instance_data()?;
                    let mut d = rfd::FileDialog::new()
                        .add_filter("Gyroflow project files", &["gyroflow"])
                        .add_filter("Video files", &["mp4", "mov", "mxf", "braw", "r3d", "insv"]);
                    let current_path = instance_data.param_project_path.get_value()?;
                    if !current_path.is_empty() {
                        if let Some(path) = std::path::Path::new(&current_path).parent() {
                            d = d.set_directory(path);
                        }
                    }
                    if let Some(d) = d.pick_file() {
                        instance_data.param_project_path.set_value(d.display().to_string())?;
                    }
                }
                if in_args.get_name()? == "OpenGyroflow" {
                    if let Some(v) = gyroflow_core::util::get_setting("exeLocation") {
                        if !v.is_empty() {
                            let project = effect.get_instance_data::<InstanceData>()?.param_project_path.get_value()?;
                            if !project.is_empty() {
                                let _ = std::process::Command::new(v)
                                    .args(["--open", &project])
                                    .spawn();
                            } else {
                                let _ = std::process::Command::new(v).spawn();
                            }
                        }
                    } else {
                        rfd::MessageDialog::new().set_description("Unable to find Gyroflow app path. Make sure to run Gyroflow app at least once and that version is at least v1.4.3").show();
                    }
                }
                if in_args.get_name()? == "OpenRecentProject" {
                    if let Some(v) = gyroflow_core::util::get_setting("lastProject") {
                        if !v.is_empty() {
                            let instance_data: &mut InstanceData = effect.get_instance_data()?;
                            instance_data.param_project_path.set_value(v)?;
                        }
                    }
                }
                if in_args.get_name()? == "gyrodata" || in_args.get_name()? == "ReloadProject" || in_args.get_name()? == "DontDrawOutside" {
                    effect.get_instance_data::<InstanceData>()?.gyrodata.clear();
                }
                if in_args.get_name()? == "LoadCurrent" {
                    let instance_data: &mut InstanceData = effect.get_instance_data()?;
                    CurrentFileInfo::query(instance_data.current_file_info.clone(), instance_data.current_file_info_pending.clone());
                }


                if in_args.get_name()? == "ToggleOverview" && in_args.get_change_reason()? == Change::UserEdited {
                    let instance_data: &mut InstanceData = effect.get_instance_data()?;

                    let on = instance_data.param_toggle_overview.get_value()?;
                    for (_, v) in instance_data.gyrodata.iter_mut() {
                        v.set_fov_overview(on);
                        v.recompute_undistortion();
                    }
                }

                OK
            }

            GetRegionOfDefinition(ref mut effect, ref in_args, ref mut out_args) => {
                let time = in_args.get_time()?;
                let instance_data = effect.get_instance_data::<InstanceData>()?;
                let rod = instance_data.source_clip.get_region_of_definition(time)?;
                let mut out_rod = rod;
                if instance_data.original_output_size != (0, 0) && !instance_data.param_dont_draw_outside.get_value_at_time(time)? {
                    out_rod.x2 = instance_data.original_output_size.0 as f64;
                    out_rod.y2 = instance_data.original_output_size.1 as f64;
                }
                out_args.set_effect_region_of_definition(out_rod)?;

                OK
            }

            DestroyInstance(ref mut effect) => {
                effect.get_instance_data::<InstanceData>()?.gyrodata.clear();
                OK
            },
            PurgeCaches(ref mut effect) => {
                effect.get_instance_data::<InstanceData>()?.gyrodata.clear();
                OK
            },

            DescribeInContext(ref mut effect, ref _in_args) => {
                let mut output_clip = effect.new_output_clip()?;
                output_clip.set_supported_components(&[ImageComponent::RGBA])?;

                let mut input_clip = effect.new_simple_input_clip()?;
                input_clip.set_supported_components(&[ImageComponent::RGBA])?;

                let mut param_set = effect.parameter_set()?;

                {
                    param_set.param_define_group("ProjectGroup")?
                             .set_label("Gyroflow project")?;

                    let mut param = param_set.param_define_string("ProjectData")?;
                    let _ = param.set_script_name("ProjectData");
                    param.set_secret(true)?;

                    if CurrentFileInfo::is_available() {
                        let mut param = param_set.param_define_button("LoadCurrent")?;
                        param.set_label("Load for current file")?;
                        param.set_hint("Try to load project file for current video file, or try to stabilize that video file directly")?;
                        param.set_parent("ProjectGroup")?;
                    }

                    let mut param = param_set.param_define_string("gyrodata")?;
                    param.set_string_type(ParamStringType::SingleLine)?;
                    param.set_label("Project file")?;
                    param.set_hint("Project file")?;
                    let _ = param.set_script_name("gyrodata");
                    param.set_parent("ProjectGroup")?;

                    let mut param = param_set.param_define_button("Browse")?;
                    param.set_label("Browse")?;
                    param.set_hint("Browse for the Gyroflow project file")?;
                    param.set_parent("ProjectGroup")?;

                    let mut param = param_set.param_define_button("OpenGyroflow")?;
                    param.set_label("Open Gyroflow")?;
                    param.set_hint("Open project in Gyroflow")?;
                    param.set_parent("ProjectGroup")?;

                    let mut param = param_set.param_define_button("ReloadProject")?;
                    param.set_label("Reload project")?;
                    param.set_hint("Reload currently loaded project")?;
                    param.set_parent("ProjectGroup")?;

                    let mut param = param_set.param_define_button("OpenRecentProject")?;
                    param.set_label("Last saved project")?;
                    param.set_hint("Load most recently saved project in the Gyroflow app")?;
                    param.set_parent("ProjectGroup")?;
                }

                {
                    param_set.param_define_group("AdjustGroup")?
                             .set_label("Adjust parameters")?;

                    let mut param = param_set.param_define_double("FOV")?;
                    param.set_double_type(ParamDoubleType::Plain)?;
                    param.set_default(1.0)?;
                    param.set_display_min(0.1)?;
                    param.set_display_max(3.0)?;
                    param.set_label("FOV")?;
                    param.set_hint("FOV")?;
                    let _ = param.set_script_name("FOV");
                    param.set_parent("AdjustGroup")?;

                    let mut param = param_set.param_define_double("Smoothness")?;
                    param.set_double_type(ParamDoubleType::Plain)?;
                    param.set_default(0.5)?;
                    param.set_display_min(0.01)?;
                    param.set_display_max(3.0)?;
                    param.set_label("Smoothness")?;
                    param.set_hint("Smoothness")?;
                    let _ = param.set_script_name("Smoothness");
                    param.set_parent("AdjustGroup")?;

                    let mut param = param_set.param_define_double("LensCorrectionStrength")?;
                    param.set_double_type(ParamDoubleType::Plain)?;
                    param.set_default(100.0)?;
                    param.set_display_min(0.0)?;
                    param.set_display_max(100.0)?;
                    param.set_label("Lens correction")?;
                    param.set_hint("Lens correction")?;
                    let _ = param.set_script_name("LensCorrectionStrength");
                    param.set_parent("AdjustGroup")?;

                    let mut param = param_set.param_define_boolean("DisableStretch")?;
                    param.set_label("Disable Gyroflow's stretch")?;
                    param.set_hint("If you used Input stretch in the lens profile in Gyroflow, and you de-stretched the video separately in Resolve, check this to disable Gyroflow's internal stretching.")?;
                    let _ = param.set_script_name("DisableStretch");
                    param.set_parent("AdjustGroup")?;

                    let mut param = param_set.param_define_boolean("DisableRotation")?;
                    param.set_label("Disable Gyroflow's rotation")?;
                    param.set_hint("If your video has rotation metadata, Gyroflow rotates it internally but so does the plugin, and it's rotated twice. Check this box in this case.")?;
                    let _ = param.set_script_name("DisableRotation");
                    param.set_parent("AdjustGroup")?;
                }

                let mut param = param_set.param_define_boolean("ToggleOverview")?;
                param.set_label("Stabilization overview")?;
                let _ = param.set_script_name("ToggleOverview");
                param.set_hint("Zooms out the view to see the stabilization results. Disable this before rendering.")?;

                let mut param = param_set.param_define_boolean("DontDrawOutside")?;
                param.set_label("Don't draw outside source clip")?;
                let _ = param.set_script_name("DontDrawOutside");
                param.set_hint("When clip and timeline aspect ratio don't match, draw the final image inside the source clip, instead of drawing outside it.")?;

                let mut param = param_set.param_define_boolean("Status")?;
                param.set_label("Status")?;
                param.set_hint("Status")?;
                param.set_enabled(false)?;

                param_set
                    .param_define_page("Main")?
                    .set_children(&[
                        "ProjectGroup",
                        "AdjustGroup",
                        "ToggleOverview", "Status", "DontDrawOutside"
                    ])?;

                OK
            }

            Describe(ref mut effect) => {
                // log::info!("host supports opencl: {:?}", _plugin_context.get_host().get_opencl_render_supported());
                // log::info!("host supports cuda: {:?}", _plugin_context.get_host().get_cuda_render_supported());
                // log::info!("host supports metal: {:?}", _plugin_context.get_host().get_metal_render_supported());

                let mut effect_properties: EffectDescriptor = effect.properties()?;
                effect_properties.set_grouping("Warp")?;

                effect_properties.set_label("Gyroflow")?;
                effect_properties.set_short_label("Gyroflow")?;
                effect_properties.set_long_label("Gyroflow")?;

                effect_properties.set_supported_pixel_depths(&[BitDepth::Byte, BitDepth::Short, BitDepth::Float])?;
                effect_properties.set_supported_contexts(&[ImageEffectContext::Filter])?;
                effect_properties.set_supports_tiles(false)?;

                effect_properties.set_single_instance(false)?;
                effect_properties.set_host_frame_threading(false)?;
                effect_properties.set_render_thread_safety(ImageEffectRender::FullySafe)?;
                effect_properties.set_supports_multi_resolution(true)?;
                effect_properties.set_temporal_clip_access(true)?;

                let opencl_devices = gyroflow_core::gpu::opencl::OclWrapper::list_devices();
                let wgpu_devices = gyroflow_core::gpu::wgpu::WgpuWrapper::list_devices();
                if !opencl_devices.is_empty() {
                    let _ = effect_properties.set_opencl_render_supported("true");
                }

                let _has_metal  = wgpu_devices.iter().any(|x| x.contains("(Metal)"));
                let _has_vulkan = wgpu_devices.iter().any(|x| x.contains("(Vulkan)"));
                let _has_dx12   = wgpu_devices.iter().any(|x| x.contains("(Dx12)"));

                #[cfg(any(target_os = "macos", target_os = "ios"))]
                if _has_metal { let _ = effect_properties.set_metal_render_supported("true"); }
                #[cfg(any(target_os = "windows", target_os = "linux"))]
                if _has_vulkan || _has_dx12 { let _ = effect_properties.set_cuda_render_supported("true"); }

                OK
            }

            Load => {
                log_panics::init();
                OK
            },

            _ => REPLY_DEFAULT,
        }
    }
}

impl InstanceData {
    #[allow(unused)]
    fn get_per_frame_params(&self) -> Result<PerFrameParams> {
        Ok(PerFrameParams {})
    }
}
