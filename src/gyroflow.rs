use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;

use gyroflow_core::{ StabilizationManager, stabilization::{ RGBA8, RGBA16, RGBAf, RGBAf16 }, keyframes::{ KeyframeType, KeyframeManager }, filesystem };
use gyroflow_core::gpu::{ BufferDescription, Buffers, BufferSource };
use lru::LruCache;
use ofx::*;
use parking_lot::{ Mutex, RwLock };
use super::fuscript::*;

plugin_module!(
    "nl.smslv.gyroflowofx.fisheyestab_v1",
    ApiVersion(1),
    PluginVersion(1, 2),
    GyroflowPlugin::default
);

// We should cache managers globally because it's common to have the effect applied to the same clip and cut the clip into multiple pieces
// We don't want to create a new manager for each piece of the same clip
// Cache key is specific enough
lazy_static::lazy_static! {
    static ref MANAGER_CACHE: Mutex<LruCache<String, Arc<StabilizationManager>>> = Mutex::new(LruCache::new(std::num::NonZeroUsize::new(8).unwrap()));
}

#[derive(Default)]
struct GyroflowPlugin {
	host_supports_multiple_clip_depths: Bool,
    context_initialized: bool,
}

struct KeyframableParams {
    fov: ParamHandle<Double>,
    smoothness: ParamHandle<Double>,
    lens_correction_strength: ParamHandle<Double>,
    horizon_lock_amount: ParamHandle<Double>,
    horizon_lock_roll: ParamHandle<Double>,
    positionx: ParamHandle<Double>,
    positiony: ParamHandle<Double>,
    rotation: ParamHandle<Double>,
    video_speed: ParamHandle<Double>,
    use_gyroflows_keyframes: ParamHandle<Bool>,
    use_gyroflows_cached: bool,

    cached_keyframes: KeyframeManager
}
unsafe impl Send for KeyframableParams { }
unsafe impl Sync for KeyframableParams { }

impl KeyframableParams {
    pub fn cache_keyframes(&mut self, num_frames: usize, fps: f64) {
        self.cached_keyframes.clear();
        self.use_gyroflows_cached = self.use_gyroflows_keyframes.get_value().unwrap_or_default();
        macro_rules! cache_key {
            ($typ:expr, $param:expr, $scale:expr) => {
                if $param.get_num_keys().unwrap_or_default() > 0 {
                    for t in 0..num_frames {
                        let time = t as f64;
                        let timestamp_us = ((time / fps * 1_000_000.0)).round() as i64;

                        if let Ok(v) = $param.get_value_at_time(time) {
                            self.cached_keyframes.set(&$typ, timestamp_us, v / $scale);
                        }
                    }
                } else {
                    if let Ok(v) = $param.get_value() {
                        self.cached_keyframes.set(&$typ, 0, v / $scale);
                    }
                }
            };
        }
        cache_key!(KeyframeType::Fov,                       self.fov,                      1.0);
        cache_key!(KeyframeType::SmoothingParamSmoothness,  self.smoothness,               1.0);
        cache_key!(KeyframeType::LensCorrectionStrength,    self.lens_correction_strength, 100.0);
        cache_key!(KeyframeType::LockHorizonAmount,         self.horizon_lock_amount,      1.0);
        cache_key!(KeyframeType::LockHorizonRoll,           self.horizon_lock_roll,        1.0);
        cache_key!(KeyframeType::VideoSpeed,                self.video_speed,              100.0);
        cache_key!(KeyframeType::VideoRotation,             self.rotation,                 1.0);
        cache_key!(KeyframeType::ZoomingCenterX,            self.positionx,                100.0);
        cache_key!(KeyframeType::ZoomingCenterY,            self.positiony,                100.0);
    }
}

#[allow(unused)]
struct InstanceData {
    source_clip: ClipInstance,
    output_clip: ClipInstance,

    keyframable_params: Arc<RwLock<KeyframableParams>>,

    param_instance_id: ParamHandle<String>,
    param_project_data: ParamHandle<String>,
    param_embedded_lens: ParamHandle<String>,
    param_embedded_preset: ParamHandle<String>,
    param_project_path: ParamHandle<String>,
    param_disable_stretch: ParamHandle<Bool>,
    param_status: ParamHandle<Bool>,
    param_open_in_gyroflow: ParamHandle<Bool>,
    param_toggle_overview: ParamHandle<Bool>,
    param_reload_project: ParamHandle<Bool>,
    param_dont_draw_outside: ParamHandle<Bool>,
    param_include_project_data: ParamHandle<Bool>,
    param_input_rotation: ParamHandle<Double>,
    gyrodata: LruCache<String, Arc<StabilizationManager>>,

    reload_values_from_project: bool,

    original_video_size: (usize, usize),
    original_output_size: (usize, usize),
    num_frames: usize,
    fps: f64,
    ever_changed: bool,

    current_file_info_pending: Arc<AtomicBool>,
    current_file_info: Arc<Mutex<Option<CurrentFileInfo>>>
}
impl Drop for InstanceData {
    fn drop(&mut self) {
        self.clear_stab();
    }
}

impl InstanceData {
    fn update_loaded_state(&mut self, loaded: bool) {
        let mut kparams = self.keyframable_params.write();
        let _ = kparams.fov.set_enabled(loaded);
        let _ = kparams.smoothness.set_enabled(loaded);
        let _ = kparams.lens_correction_strength.set_enabled(loaded);
        let _ = kparams.horizon_lock_amount.set_enabled(loaded);
        let _ = kparams.horizon_lock_roll.set_enabled(loaded);
        let _ = kparams.positionx.set_enabled(loaded);
        let _ = kparams.positiony.set_enabled(loaded);
        let _ = kparams.rotation.set_enabled(loaded);
        let _ = kparams.video_speed.set_enabled(loaded);
        let _ = self.param_disable_stretch.set_enabled(loaded);
        let _ = self.param_toggle_overview.set_enabled(loaded);
        let _ = self.param_reload_project.set_enabled(loaded);
        let _ = self.param_status.set_label(if loaded { "OK" } else { "Project not loaded" });
        let _ = self.param_status.set_value(loaded);
        let _ = self.param_open_in_gyroflow.set_label(if loaded { "Open in Gyroflow" } else { "Open Gyroflow" });
    }

    fn set_keyframe_provider(&self, stab: &StabilizationManager) {
        let kparams = self.keyframable_params.clone();
        stab.keyframes.write().set_custom_provider(move |kf, typ, timestamp_ms| -> Option<f64> {
            let params = kparams.read();
            if params.use_gyroflows_cached && kf.is_keyframed_internally(typ) { return None; }
            params.cached_keyframes.value_at_video_timestamp(typ, timestamp_ms)
        });
    }

    fn gyrodata(&mut self, bit_depth: BitDepth, output_rect: RectI, loading_pending_video_file: bool) -> Result<Arc<StabilizationManager>> {
        let disable_stretch = self.param_disable_stretch.get_value()?;

        let source_rect = self.source_clip.get_region_of_definition(0.0)?;
        let mut source_rect = RectI {
            x1: source_rect.x1 as i32,
            x2: source_rect.x2 as i32,
            y1: source_rect.y1 as i32,
            y2: source_rect.y2 as i32
        };
        if source_rect.x1 != output_rect.x1 || source_rect.x2 != output_rect.x2 || source_rect.y1 != output_rect.y1 || source_rect.y2 != output_rect.y2 {
            source_rect = self.source_clip.get_image(0.0)?.get_bounds()?;
        }
        let in_size = ((source_rect.x2 - source_rect.x1) as usize, (source_rect.y2 - source_rect.y1) as usize);
        let out_size = ((output_rect.x2 - output_rect.x1) as usize, (output_rect.y2 - output_rect.y1) as usize);

        let instance_id = self.param_instance_id.get_value()?;
        let path = self.param_project_path.get_value()?;
        if path.is_empty() {
            self.update_loaded_state(false);
            return Err(Error::UnknownError);
        }
        let key = format!("{path}{bit_depth:?}{in_size:?}{out_size:?}{disable_stretch}{instance_id}");
        let cloned = MANAGER_CACHE.lock().get(&key).map(Arc::clone);
        let stab = if let Some(stab) = cloned {
            // Cache it in this instance as well
            if !self.gyrodata.contains(&key) {
                self.gyrodata.put(key.to_owned(), stab.clone());
            }
            self.set_keyframe_provider(&stab);
            stab
        } else {
            let mut stab = StabilizationManager::default();
            {
                // Find first lens profile database with loaded profiles
                let lock = MANAGER_CACHE.lock();
                for (_, v) in lock.iter() {
                    if v.lens_profile_db.read().loaded {
                        stab.lens_profile_db = v.lens_profile_db.clone();
                        break;
                    }
                }
            }

            if !path.ends_with(".gyroflow") {
                // Try to load from video file
                // let mut metadata = None;
                // if path.to_ascii_lowercase().ends_with(".mxf") || path.to_ascii_lowercase().ends_with(".braw") {
                //     let lock = self.current_file_info.lock();
                //     if let Some(ref current_file) = *lock {
                //         metadata = Some(VideoMetadata {
                //             duration_s: current_file.duration_s,
                //             fps: current_file.fps,
                //             width: current_file.width,
                //             height: current_file.height,
                //             rotation: 0
                //         });
                //     }
                // }

                match stab.load_video_file(&filesystem::path_to_url(&path), None) {
                    Ok(md) => {
                        if let Ok(d) = self.param_embedded_lens.get_value() {
                            if !d.is_empty() {
                                if let Err(e) = stab.load_lens_profile(&d) {
                                    rfd::MessageDialog::new()
                                        .set_description(&format!("Failed to load lens profile: {e:?}"))
                                        .show();
                                }
                            }
                        }
                        if let Ok(d) = self.param_embedded_preset.get_value() {
                            if !d.is_empty() {
                                let mut is_preset = false;
                                if let Err(e) = stab.import_gyroflow_data(d.as_bytes(), true, None, |_|(), Arc::new(AtomicBool::new(false)), &mut is_preset) {
                                    rfd::MessageDialog::new()
                                        .set_description(&format!("Failed to load preset: {e:?}"))
                                        .show();
                                }
                            }
                        }
                        if self.param_include_project_data.get_value()? {
                            if let Ok(data) = stab.export_gyroflow_data(gyroflow_core::GyroflowProjectType::WithGyroData, "{}", None) {
                                self.param_project_data.set_value(data)?;
                            }
                        }
                        if md.rotation != 0 {
                            let r = ((360 - md.rotation) % 360) as f64;
                            self.param_input_rotation.set_value(r)?;
                            stab.params.write().video_rotation = r;
                        }
                        if !stab.gyro.read().file_metadata.has_accurate_timestamps && loading_pending_video_file {
                            self.open_gyroflow();
                        }
                    },
                    Err(e) => {
                        let embedded_data = self.param_project_data.get_value()?;
                        if !embedded_data.is_empty() {
                            let mut is_preset = false;
                            stab.import_gyroflow_data(embedded_data.as_bytes(), true, None, |_|(), Arc::new(AtomicBool::new(false)), &mut is_preset).map_err(|e| {
                                log::error!("load_gyro_data error: {}", &e);
                                self.update_loaded_state(false);
                                Error::UnknownError
                            })?;
                        } else {
                            log::error!("An error occured: {e:?}");
                            self.update_loaded_state(false);
                            self.param_status.set_label("Failed to load file info!")?;
                            self.param_status.set_hint(&format!("Error loading {path}: {e:?}."))?;
                            if loading_pending_video_file {
                                self.open_gyroflow();
                            }
                            return Err(Error::UnknownError);
                        }
                    }
                }
            } else {
                let project_data = {
                    if let Ok(data) = std::fs::read_to_string(&path) {
                        if self.param_include_project_data.get_value()? {
                            self.param_project_data.set_value(data.clone())?;
                        } else {
                            self.param_project_data.set_value("".to_string())?;
                        }
                        data
                    } else {
                        self.param_project_data.get_value()?
                    }
                };
                let mut is_preset = false;
                stab.import_gyroflow_data(project_data.as_bytes(), true, Some(&filesystem::path_to_url(&path)), |_|(), Arc::new(AtomicBool::new(false)), &mut is_preset).map_err(|e| {
                    log::error!("load_gyro_data error: {}", &e);
                    self.update_loaded_state(false);
                    Error::UnknownError
                })?;
            }

            let loaded = {
                stab.params.write().calculate_ramped_timestamps(&stab.keyframes.read(), false, true);
                let params = stab.params.read();
                self.original_video_size = params.video_size;
                self.original_output_size = params.video_output_size;
                self.num_frames = params.frame_count;
                self.fps = params.fps;
                let loaded = params.duration_ms > 0.0;
                if loaded && self.reload_values_from_project {
                    self.reload_values_from_project = false;
                    let smooth = stab.smoothing.read();
                    let smoothness = smooth.current().get_parameter("smoothness");

                    let kparams = self.keyframable_params.read();
                    kparams.fov.set_value(params.fov)?;
                    kparams.smoothness.set_value(smoothness)?;
                    kparams.lens_correction_strength.set_value((params.lens_correction_amount * 100.0).min(100.0))?;
                    kparams.horizon_lock_amount.set_value(if smooth.horizon_lock.lock_enabled { smooth.horizon_lock.horizonlockpercent } else { 0.0 })?;
                    kparams.horizon_lock_roll.set_value(if smooth.horizon_lock.lock_enabled { smooth.horizon_lock.horizonroll } else { 0.0 })?;
                    kparams.video_speed.set_value(params.video_speed * 100.0)?;
                    kparams.positionx.set_value(params.adaptive_zoom_center_offset.0 * 100.0)?;
                    kparams.positiony.set_value(params.adaptive_zoom_center_offset.1 * 100.0)?;
                    kparams.rotation.set_value(params.video_rotation)?;

                    let keyframes = stab.keyframes.read();
                    let all_keys = keyframes.get_all_keys();
                    kparams.use_gyroflows_keyframes.set_value(!all_keys.is_empty())?;
                    for k in all_keys {
                        if let Some(keys) = keyframes.get_keyframes(k) {
                            if !keys.is_empty() {
                                macro_rules! set_keys {
                                    ($name:expr, $scale:expr) => {
                                        $name.delete_all_keys()?;
                                        for (ts, v) in keys {
                                            let ts = if k == &KeyframeType::VideoSpeed { params.get_source_timestamp_at_ramped_timestamp(*ts) } else { *ts };
                                            let time = (((ts as f64 / 1000.0) * params.fps) / 1000.0).round();
                                            $name.set_value_at_time(time, v.value * $scale)?;
                                        }
                                    };
                                }
                                match k {
                                    KeyframeType::Fov                      => { set_keys!(kparams.fov,                      1.0); },
                                    KeyframeType::SmoothingParamSmoothness => { set_keys!(kparams.smoothness,               1.0); },
                                    KeyframeType::LensCorrectionStrength   => { set_keys!(kparams.lens_correction_strength, 100.0); },
                                    KeyframeType::LockHorizonAmount        => { set_keys!(kparams.horizon_lock_amount,      1.0); },
                                    KeyframeType::LockHorizonRoll          => { set_keys!(kparams.horizon_lock_roll,        1.0); },
                                    KeyframeType::VideoSpeed               => { set_keys!(kparams.video_speed,              100.0); },
                                    KeyframeType::VideoRotation            => { set_keys!(kparams.rotation,                 1.0); },
                                    KeyframeType::ZoomingCenterX           => { set_keys!(kparams.positionx,                100.0); },
                                    KeyframeType::ZoomingCenterY           => { set_keys!(kparams.positiony,                100.0); },
                                    _ => { }
                                }
                            }
                        }
                    }
                }
                self.keyframable_params.write().cache_keyframes(self.num_frames, self.fps.max(1.0));
                loaded
            };

            self.update_loaded_state(loaded);

            if disable_stretch {
                stab.disable_lens_stretch();
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
                stab.share_wgpu_instances = true;
                stab.interpolation = gyroflow_core::stabilization::Interpolation::Lanczos4;
            }

            self.set_keyframe_provider(&stab);

            stab.invalidate_smoothing();
            stab.recompute_blocking();
            let inverse = !(self.keyframable_params.read().use_gyroflows_keyframes.get_value()? && stab.keyframes.read().is_keyframed_internally(&KeyframeType::VideoSpeed));
            stab.params.write().calculate_ramped_timestamps(&stab.keyframes.read(), inverse, inverse);

            let stab = Arc::new(stab);
            // Insert to static global cache
            MANAGER_CACHE.lock().put(key.to_owned(), stab.clone());
            // Cache it in this instance as well
            self.gyrodata.put(key.to_owned(), stab.clone());

            stab
        };

        Ok(stab)
    }

    pub fn check_pending_file_info(&mut self) -> Result<bool> { // -> is_video_file
        if self.current_file_info_pending.load(SeqCst) {
            self.current_file_info_pending.store(false, SeqCst);
            let lock = self.current_file_info.lock();
            if let Some(ref current_file) = *lock {
                if let Some(proj) = &current_file.project_path {
                    self.param_project_path.set_value(proj.to_string())?;
                } else {
                    // Try to use the video directly
                    self.param_project_path.set_value(current_file.file_path.clone())?;
                    return Ok(true);
                }
            }
        }
        Ok(false)
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

    pub fn clear_stab(&mut self) {
        let local_keys = self.gyrodata.iter().map(|x| x.0.clone()).collect::<Vec<_>>();
        self.gyrodata.clear();

        // If there are no more local references, delete it from global cache
        let mut lock = MANAGER_CACHE.lock();
        for key in local_keys {
            if let Some(v) = lock.get(&key) {
                if Arc::strong_count(v) == 1 {
                    lock.pop(&key);
                }
            }
        }
    }

    pub fn open_gyroflow(&self) {
        if let Some(v) = gyroflow_core::util::get_setting("exeLocation") {
            if !v.is_empty() {
                if let Ok(project) = self.param_project_path.get_value() {
                    if !project.is_empty() {
                        if cfg!(target_os = "macos") {
                            let _ = std::process::Command::new("open").args(["-a", &v, "--args", "--open", &project]).spawn();
                        } else {
                            let _ = std::process::Command::new(v).args(["--open", &project]).spawn();
                        }
                    } else {
                        if cfg!(target_os = "macos") {
                            let _ = std::process::Command::new("open").args(["-a", &v]).spawn();
                        } else {
                            let _ = std::process::Command::new(v).spawn();
                        }
                    }
                }
            }
        } else {
            rfd::MessageDialog::new().set_description("Unable to find Gyroflow app path. Make sure to run Gyroflow app at least once and that version is at least v1.4.3").show();
        }
    }
}

struct PerFrameParams { }

impl Execute for GyroflowPlugin {
    #[allow(clippy::float_cmp)]
    fn execute(&mut self, _plugin_context: &PluginContext, action: &mut Action) -> Result<Int> {
        use Action::*;

        match *action {
            Render(ref mut effect, ref in_args) => {
                let _time = std::time::Instant::now();

                let time = in_args.get_time()?;
                let instance_data: &mut InstanceData = effect.get_instance_data()?;

                let loading_pending_video_file = instance_data.check_pending_file_info()?;

                let output_image = if in_args.get_opengl_enabled().unwrap_or_default() {
                    instance_data.output_clip.load_texture_mut(time, None)?
                } else {
                    instance_data.output_clip.get_image_mut(time)?
                };
                let output_image = output_image.borrow_mut();

                let output_rect: RectI = output_image.get_region_of_definition()?;

                let stab = instance_data.gyrodata(output_image.get_pixel_depth()?, output_rect, loading_pending_video_file)?;

                let params = stab.params.read();
                let fps = params.fps;
                let src_fps = instance_data.source_clip.get_frame_rate().unwrap_or(fps);
                let org_ratio = params.video_size.0 as f64 / params.video_size.1 as f64;
                let (has_accurate_timestamps, has_offsets) = {
                    let gyro = stab.gyro.read();
                    (gyro.file_metadata.has_accurate_timestamps, !gyro.get_offsets().is_empty())
                };

                let frame_number = (params.frame_count - 1) as f64;

                let mut speed_stretch = 1.0;
                if let Ok(range) = instance_data.source_clip.get_frame_range() {
                    if range.max > 0.0 {
                        if (frame_number - range.max).abs() > 2.0 {
                            speed_stretch = ((frame_number / range.max) * 100.0).round() / 100.0;
                        }
                    }
                }
                if (src_fps - fps).abs() > 0.01 {
                    instance_data.param_status.set_label("Timeline fps mismatch!")?;
                    instance_data.param_status.set_hint("Timeline frame rate doesn't match the clip frame rate!")?;
                    if instance_data.param_status.get_value()? {
                        instance_data.param_status.set_value(false)?;
                    }
                } else if !has_accurate_timestamps && !has_offsets {
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
                        instance_data.update_loaded_state(true);
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

                let source_image = if in_args.get_opengl_enabled().unwrap_or_default() {
                    instance_data.source_clip.load_texture(time, None)?
                } else {
                    instance_data.source_clip.get_image(time)?
                };

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
                if (out_scale.x != 1.0 || out_scale.y != 1.0) && !in_args.get_opengl_enabled().unwrap_or_default() {
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

                let input_rotation = instance_data.param_input_rotation.get_value_at_time(time).ok().map(|x| x as f32);

                // log::debug!("src_size: {src_size:?} | src_rect: {src_rect:?}");
                // log::debug!("out_size: {out_size:?} | out_rect: {out_rect:?}");

                let mut buffers =
                    if in_args.get_opencl_enabled().unwrap_or_default() {
                        use std::ffi::c_void;
                        let queue = in_args.get_opencl_command_queue()? as *mut c_void;
                        Some(Buffers {
                            input: BufferDescription {
                                size: src_size,
                                rect: Some(src_rect),
                                data: BufferSource::OpenCL { texture: source_image.get_data()? as *mut c_void, queue },
                                rotation: input_rotation,
                                texture_copy: false
                            },
                            output: BufferDescription {
                                size: out_size,
                                rect: out_rect,
                                data: BufferSource::OpenCL { texture: output_image.get_data()? as *mut c_void, queue },
                                rotation: None,
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
                                    rotation: input_rotation,
                                    texture_copy: false
                                },
                                output: BufferDescription {
                                    size: out_size,
                                    rect: out_rect,
                                    data: BufferSource::MetalBuffer { buffer: out_ptr, command_queue },
                                    rotation: None,
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
                                    rotation: input_rotation,
                                    texture_copy: true
                                },
                                output: BufferDescription {
                                    size: out_size,
                                    rect: out_rect,
                                    data: BufferSource::CUDABuffer { buffer: out_ptr },
                                    rotation: None,
                                    texture_copy: true
                                }
                            })
                        }
                    } else if in_args.get_opengl_enabled().unwrap_or_default() {
                        let texture = source_image.get_opengl_texture_index()? as u32;
                        let out_texture = output_image.get_opengl_texture_index()? as u32;
                        let mut src_size = src_size;
                        let mut out_size = out_size;
                        src_size.2 = src_size.0 * 4 * match source_image.get_pixel_depth()? { BitDepth::None => 0, BitDepth::Byte => 1, BitDepth::Short => 2, BitDepth::Half => 2, BitDepth::Float => 4 };
                        out_size.2 = out_size.0 * 4 * match output_image.get_pixel_depth()? { BitDepth::None => 0, BitDepth::Byte => 1, BitDepth::Short => 2, BitDepth::Half => 2, BitDepth::Float => 4 };

                        // log::info!("OpenGL in: {texture}, out: {out_texture} src_size: {src_size:?}, out_size: {out_size:?}, in_rect: {src_rect:?}, out_rect: {out_rect:?}");
                        Some(Buffers {
                            input: BufferDescription {
                                size: src_size,
                                rect: Some(src_rect),
                                data: BufferSource::OpenGL { texture: texture, context: std::ptr::null_mut() },
                                rotation: input_rotation,
                                texture_copy: true
                            },
                            output: BufferDescription {
                                size: out_size,
                                rect: out_rect,
                                data: BufferSource::OpenGL { texture: out_texture, context: std::ptr::null_mut() },
                                rotation: None,
                                texture_copy: true
                            }
                        })
                    } else {
                        use std::slice::from_raw_parts_mut;
                        let src_buf = unsafe { match source_image.get_pixel_depth()? {
                            BitDepth::None  => { return FAILED; }
                            BitDepth::Byte  => { let b = source_image.get_descriptor::<RGBAColourB>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) },
                            BitDepth::Short => { let b = source_image.get_descriptor::<RGBAColourS>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) },
                            BitDepth::Half  => { let b = source_image.get_descriptor::<RGBAColourS>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) },
                            BitDepth::Float => { let b = source_image.get_descriptor::<RGBAColourF>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) }
                        } };
                        let dst_buf = unsafe { match output_image.get_pixel_depth()? {
                            BitDepth::None  => { return FAILED; }
                            BitDepth::Byte  => { let b = output_image.get_descriptor::<RGBAColourB>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) },
                            BitDepth::Short => { let b = output_image.get_descriptor::<RGBAColourS>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) },
                            BitDepth::Half  => { let b = output_image.get_descriptor::<RGBAColourS>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) },
                            BitDepth::Float => { let b = output_image.get_descriptor::<RGBAColourF>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) }
                        } };

                        Some(Buffers {
                            input: BufferDescription {
                                size: src_size,
                                rect: Some(src_rect),
                                data: BufferSource::Cpu { buffer: src_buf },
                                rotation: input_rotation,
                                texture_copy: false
                            },
                            output: BufferDescription {
                                size: out_size,
                                rect: out_rect,
                                data: BufferSource::Cpu { buffer: dst_buf },
                                rotation: None,
                                texture_copy: false
                            }
                        })
                    };

                if effect.abort()? { return FAILED; }

                if let Some(ref mut buffers) = buffers {
                    let processed = match output_image.get_pixel_depth()? {
                        BitDepth::None  => { return FAILED; },
                        BitDepth::Byte  => stab.process_pixels::<RGBA8>  (timestamp_us, buffers),
                        BitDepth::Short => stab.process_pixels::<RGBA16> (timestamp_us, buffers),
                        BitDepth::Half  => stab.process_pixels::<RGBAf16>(timestamp_us, buffers),
                        BitDepth::Float => stab.process_pixels::<RGBAf>  (timestamp_us, buffers)
                    };
                    match processed {
                        Ok(_) => {
                            // log::info!("Rendered | {}x{} in {:.2}ms: {:?}", src_size.0, src_size.1, _time.elapsed().as_micros() as f64 / 1000.0, _);
                            OK
                        },
                        Err(e) => {
                            log::warn!("Failed to render: {e:?}");
                            FAILED
                        }
                    }
                } else {
                    FAILED
                }
            }

            CreateInstance(ref mut effect) => {
                let param_set = effect.parameter_set()?;
                // let mut effect_props: EffectInstance = effect.properties()?;

                let source_clip = effect.get_simple_input_clip()?;
                let output_clip = effect.get_output_clip()?;

                let mut instance_data = InstanceData {
                    source_clip,
                    output_clip,
                    param_instance_id:              param_set.parameter("InstanceId")?,
                    param_project_data:             param_set.parameter("ProjectData")?,
                    param_embedded_lens:            param_set.parameter("EmbeddedLensProfile")?,
                    param_embedded_preset:          param_set.parameter("EmbeddedPreset")?,
                    param_project_path:             param_set.parameter("gyrodata")?,
                    param_disable_stretch:          param_set.parameter("DisableStretch")?,
                    param_status:                   param_set.parameter("Status")?,
                    param_open_in_gyroflow:         param_set.parameter("OpenGyroflow")?,
                    param_reload_project:           param_set.parameter("ReloadProject")?,
                    param_toggle_overview:          param_set.parameter("ToggleOverview")?,
                    param_dont_draw_outside:        param_set.parameter("DontDrawOutside")?,
                    param_include_project_data:     param_set.parameter("IncludeProjectData")?,
                    param_input_rotation:           param_set.parameter("InputRotation")?,
                    gyrodata:                       LruCache::new(std::num::NonZeroUsize::new(20).unwrap()),
                    original_output_size:           (0, 0),
                    original_video_size:            (0, 0),
                    num_frames:                     0,
                    fps:                            0.0,
                    current_file_info:              Arc::new(Mutex::new(None)),
                    current_file_info_pending:      Arc::new(AtomicBool::new(false)),
                    reload_values_from_project:     false,
                    ever_changed: false,
                    keyframable_params: Arc::new(RwLock::new(KeyframableParams {
                        fov:                      param_set.parameter("FOV")?,
                        smoothness:               param_set.parameter("Smoothness")?,
                        lens_correction_strength: param_set.parameter("LensCorrectionStrength")?,
                        horizon_lock_amount:      param_set.parameter("HorizonLockAmount")?,
                        horizon_lock_roll:        param_set.parameter("HorizonLockRoll")?,
                        video_speed:              param_set.parameter("VideoSpeed")?,
                        positionx:                param_set.parameter("PositionX")?,
                        positiony:                param_set.parameter("PositionY")?,
                        rotation:                 param_set.parameter("Rotation")?,
                        use_gyroflows_keyframes:  param_set.parameter("UseGyroflowsKeyframes")?,
                        use_gyroflows_cached:     param_set.parameter::<Bool>("UseGyroflowsKeyframes")?.get_value()?,
                        cached_keyframes:         KeyframeManager::default()
                    })),
                };
                if instance_data.param_instance_id.get_value()?.is_empty() {
                    instance_data.ever_changed = true;
                    instance_data.param_instance_id.set_value(format!("{}", fastrand::u64(..)))?;
                }

                effect.set_instance_data(instance_data)?;

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
                if in_args.get_name()? == "LoadLens" {
                    let instance_data: &mut InstanceData = effect.get_instance_data()?;
                    let lens_directory = || -> Option<std::path::PathBuf> {
                        let exe = gyroflow_core::util::get_setting("exeLocation").filter(|x| !x.is_empty())?;
                        if cfg!(target_os = "macos") {
                            let mut path = std::path::Path::new(&exe).to_path_buf();
                            path.push("Contents");
                            path.push("Resources");
                            path.push("camera_presets");
                            Some(path.into())
                        } else {
                            let mut path = std::path::Path::new(&exe).parent()?.to_path_buf();
                            path.push("camera_presets");
                            Some(path.into())
                        }
                    }();
                    log::info!("lens directory: {lens_directory:?}");

                    let mut d = rfd::FileDialog::new().add_filter("Lens profiles and presets", &["json", "gyroflow"]);
                    if let Some(dir) = lens_directory {
                        d = d.set_directory(dir);
                    }
                    if let Some(d) = d.pick_file() {
                        let d = d.display().to_string();
                        if !d.is_empty() {
                            if let Ok(contents) = std::fs::read_to_string(&d) {
                                if d.ends_with(".json") {
                                    instance_data.param_embedded_lens.set_value(contents)?;
                                } else {
                                    instance_data.param_embedded_preset.set_value(contents)?;
                                }
                            }
                            instance_data.clear_stab();
                        }
                    }
                }
                if in_args.get_name()? == "OpenGyroflow" {
                    effect.get_instance_data::<InstanceData>()?.open_gyroflow();
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
                    let instance_data = effect.get_instance_data::<InstanceData>()?;
                    if in_args.get_name()? == "gyrodata" || in_args.get_name()? == "ReloadProject" {
                        instance_data.reload_values_from_project = true;
                    }
                    instance_data.clear_stab();
                }
                if in_args.get_name()? == "IncludeProjectData" {
                    let instance_data = effect.get_instance_data::<InstanceData>()?;
                    let path = instance_data.param_project_path.get_value()?;
                    if instance_data.param_include_project_data.get_value()? {
                        if path.ends_with(".gyroflow") {
                            if let Ok(data) = std::fs::read_to_string(&path) {
                                if !data.contains("\"raw_imu\": null") || !data.contains("\"quaternions\": null") {
                                    instance_data.param_project_data.set_value(data.clone())?;
                                } else {
                                    if let Some((_, stab)) = instance_data.gyrodata.peek_lru() {
                                        if let Ok(data) = stab.export_gyroflow_data(gyroflow_core::GyroflowProjectType::WithGyroData, "{}", None) {
                                            instance_data.param_project_data.set_value(data)?;
                                        }
                                    }
                                }
                            } else {
                                instance_data.param_project_data.set_value("".to_string())?;
                            }
                        } else {
                            if let Some((_, stab)) = instance_data.gyrodata.peek_lru() {
                                if let Ok(data) = stab.export_gyroflow_data(gyroflow_core::GyroflowProjectType::WithGyroData, "{}", None) {
                                    instance_data.param_project_data.set_value(data)?;
                                }
                            }
                        }
                    } else {
                        instance_data.param_project_data.set_value("".to_string())?;
                    }
                }
                if in_args.get_name()? == "LoadCurrent" {
                    let instance_data: &mut InstanceData = effect.get_instance_data()?;
                    CurrentFileInfo::query(instance_data.current_file_info.clone(), instance_data.current_file_info_pending.clone());
                }
                if in_args.get_change_reason()? == Change::UserEdited {
                    match in_args.get_name()?.as_ref() {
                        "FOV" | "Smoothness" | "LensCorrectionStrength" |
                        "HorizonLockAmount" | "HorizonLockRoll" |
                        "PositionX" | "PositionY" | "Rotation" | "InputRotation" | "VideoSpeed" |
                        "UseGyroflowsKeyframes" | "RecalculateKeyframes" => {
                            let instance_data: &mut InstanceData = effect.get_instance_data()?;
                            instance_data.param_status.set_label("Calculating...")?;
                            if !instance_data.ever_changed {
                                instance_data.ever_changed = true;
                                instance_data.param_instance_id.set_value(format!("{}", fastrand::u64(..)))?;
                                instance_data.clear_stab();
                            }
                            instance_data.keyframable_params.write().cache_keyframes(instance_data.num_frames, instance_data.fps.max(1.0));
                            for (_, v) in instance_data.gyrodata.iter_mut() {
                                match in_args.get_name()?.as_ref() {
                                    "Smoothness" | "HorizonLockAmount" | "HorizonLockRoll" | "RecalculateKeyframes" => { v.recompute_smoothness(); v.recompute_adaptive_zoom(); },
                                    "LensCorrectionStrength" | "PositionX" | "PositionY" | "Rotation" => { v.recompute_adaptive_zoom(); },
                                    _ => { }
                                }
                                v.recompute_undistortion();
                                match in_args.get_name()?.as_ref() {
                                    "VideoSpeed" | "UseGyroflowsKeyframes" | "RecalculateKeyframes" => {
                                        let inverse = !(instance_data.keyframable_params.read().use_gyroflows_keyframes.get_value()? && v.keyframes.read().is_keyframed_internally(&KeyframeType::VideoSpeed));
                                        v.params.write().calculate_ramped_timestamps(&v.keyframes.read(), inverse, inverse);
                                    },
                                    _ => { }
                                }
                            }
                        },
                        _ => { }
                    }
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
                effect.get_instance_data::<InstanceData>()?.clear_stab();
                OK
            },
            PurgeCaches(ref mut effect) => {
                effect.get_instance_data::<InstanceData>()?.clear_stab();
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

                    param_set.param_define_string("InstanceId")?
                             .set_secret(true)?;

                    for x in ["ProjectData", "EmbeddedLensProfile", "EmbeddedPreset"] {
                        let mut param = param_set.param_define_string(x)?;
                        let _ = param.set_script_name(x);
                        param.set_secret(true)?;
                    }

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

                    let mut param = param_set.param_define_button("LoadLens")?;
                    param.set_label("Load preset/lens profile")?;
                    param.set_hint("Browse for the lens profile or a preset")?;
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

                    let mut param = param_set.param_define_boolean("Status")?;
                    param.set_label("Status")?;
                    param.set_hint("Status")?;
                    param.set_enabled(false)?;
                    param.set_parent("ProjectGroup")?;

                }

                {
                    param_set.param_define_group("AdjustGroup")?
                             .set_label("Adjust parameters")?;

                    let mut param = param_set.param_define_double("FOV")?;
                    param.set_default(1.0)?;
                    param.set_display_min(0.1)?;
                    param.set_display_max(3.0)?;
                    param.set_label("FOV")?;
                    param.set_hint("FOV")?;
                    let _ = param.set_script_name("FOV");
                    param.set_parent("AdjustGroup")?;

                    let mut param = param_set.param_define_double("Smoothness")?;
                    param.set_default(0.5)?;
                    param.set_display_min(0.01)?;
                    param.set_display_max(3.0)?;
                    param.set_label("Smoothness")?;
                    param.set_hint("Smoothness")?;
                    let _ = param.set_script_name("Smoothness");
                    param.set_parent("AdjustGroup")?;

                    let mut param = param_set.param_define_double("LensCorrectionStrength")?;
                    param.set_default(100.0)?;
                    param.set_display_min(0.0)?;
                    param.set_display_max(100.0)?;
                    param.set_label("Lens correction")?;
                    param.set_hint("Lens correction")?;
                    let _ = param.set_script_name("LensCorrectionStrength");
                    param.set_parent("AdjustGroup")?;

                    let mut param = param_set.param_define_double("HorizonLockAmount")?;
                    param.set_default(0.0)?;
                    param.set_display_min(0.0)?;
                    param.set_display_max(100.0)?;
                    param.set_label("Horizon lock")?;
                    param.set_hint("Horizon lock amount")?;
                    let _ = param.set_script_name("HorizonLockAmount");
                    param.set_parent("AdjustGroup")?;

                    let mut param = param_set.param_define_double("HorizonLockRoll")?;
                    param.set_default(0.0)?;
                    param.set_display_min(-100.0)?;
                    param.set_display_max(100.0)?;
                    param.set_label("Horizon roll")?;
                    param.set_hint("Horizon lock roll adjustment")?;
                    let _ = param.set_script_name("HorizonLockRoll");
                    param.set_parent("AdjustGroup")?;

                    let mut param = param_set.param_define_double("PositionX")?;
                    param.set_default(0.0)?;
                    param.set_display_min(-100.0)?;
                    param.set_display_max(100.0)?;
                    param.set_label("Position offset X")?;
                    let _ = param.set_script_name("PositionX");
                    param.set_parent("AdjustGroup")?;

                    let mut param = param_set.param_define_double("PositionY")?;
                    param.set_default(0.0)?;
                    param.set_display_min(-100.0)?;
                    param.set_display_max(100.0)?;
                    param.set_label("Position offset Y")?;
                    let _ = param.set_script_name("PositionY");
                    param.set_parent("AdjustGroup")?;

                    let mut param = param_set.param_define_double("InputRotation")?;
                    param.set_default(0.0)?;
                    param.set_display_min(-360.0)?;
                    param.set_display_max(360.0)?;
                    param.set_label("Input rotation")?;
                    let _ = param.set_script_name("InputRotation");
                    param.set_parent("AdjustGroup")?;

                    let mut param = param_set.param_define_double("Rotation")?;
                    param.set_default(0.0)?;
                    param.set_display_min(-360.0)?;
                    param.set_display_max(360.0)?;
                    param.set_label("Video rotation")?;
                    let _ = param.set_script_name("Rotation");
                    param.set_parent("AdjustGroup")?;

                    let mut param = param_set.param_define_double("VideoSpeed")?;
                    param.set_default(100.0)?;
                    param.set_display_min(0.0001)?;
                    param.set_display_max(1000.0)?;
                    param.set_label("Video speed")?;
                    param.set_hint("Use this slider to change video speed or keyframe it, instead of built-in speed changes in the editor")?;
                    let _ = param.set_script_name("VideoSpeed");
                    param.set_parent("AdjustGroup")?;

                    let mut param = param_set.param_define_boolean("DisableStretch")?;
                    param.set_label("Disable Gyroflow's stretch")?;
                    param.set_hint("If you used Input stretch in the lens profile in Gyroflow, and you de-stretched the video separately in Resolve, check this to disable Gyroflow's internal stretching.")?;
                    let _ = param.set_script_name("DisableStretch");
                    param.set_parent("AdjustGroup")?;
                }
                {
                    param_set.param_define_group("KeyframesGroup")?
                             .set_label("Keyframes")?;

                    let mut param = param_set.param_define_boolean("UseGyroflowsKeyframes")?;
                    param.set_label("Use Gyroflow's keyframes")?;
                    let _ = param.set_script_name("UseGyroflowsKeyframes");
                    param.set_hint("Use internal Gyroflow's keyframes, instead of the editor ones.")?;
                    param.set_parent("KeyframesGroup")?;

                    let mut param = param_set.param_define_button("RecalculateKeyframes")?;
                    param.set_label("Recalculate keyframes")?;
                    param.set_hint("Recalculate keyframes after adjusting the splines (in Fusion mode)")?;
                    param.set_parent("KeyframesGroup")?;
                }

                let mut param = param_set.param_define_boolean("ToggleOverview")?;
                param.set_label("Stabilization overview")?;
                let _ = param.set_script_name("ToggleOverview");
                param.set_hint("Zooms out the view to see the stabilization results. Disable this before rendering.")?;

                let mut param = param_set.param_define_boolean("DontDrawOutside")?;
                param.set_label("Don't draw outside source clip")?;
                let _ = param.set_script_name("DontDrawOutside");
                param.set_hint("When clip and timeline aspect ratio don't match, draw the final image inside the source clip, instead of drawing outside it.")?;

                let mut param = param_set.param_define_boolean("IncludeProjectData")?;
                param.set_label("Embed .gyroflow data in plugin")?;
                param.set_hint("If you intend to share the project to someone else, the plugin can embed the Gyroflow project data including gyro data inside the video editor project. This way you don't have to share .gyroflow project files. Enabling this option will make the project bigger.")?;

                param_set
                    .param_define_page("Main")?
                    .set_children(&[
                        "ProjectGroup",
                        "AdjustGroup",
                        "KeyframesGroup",
                        "ToggleOverview", "DontDrawOutside", "IncludeProjectData"
                    ])?;

                OK
            }

            /*GetClipPreferences(ref mut effect, ref mut out_args) => {
				let instance_data: &mut InstanceData = effect.get_instance_data()?;
				let bit_depth = instance_data.source_clip.get_pixel_depth()?;
				let image_component = instance_data.source_clip.get_components()?;
				let output_component = match image_component {
					ImageComponent::RGBA | ImageComponent::RGB => ImageComponent::RGBA,
					_ => ImageComponent::Alpha,
				};
				out_args.set_raw(image_clip_prop_components!(clip_output!()), output_component.to_bytes())?;

				if self.host_supports_multiple_clip_depths {
					out_args.set_raw(image_clip_prop_depth!(clip_output!()), bit_depth.to_bytes())?;
				}
				OK
			}*/

            OpenGLContextAttached(ref mut _effect) => {
                log::info!("OpenGLContextAttached");
				if !self.context_initialized {
                    gyroflow_core::gpu::initialize_contexts();
                    self.context_initialized = true;
                }
                OK
            },
            OpenGLContextDetached(ref mut _effect) => {
                log::info!("OpenGLContextDetached");
                OK
            },
            Describe(ref mut effect) => {
				self.host_supports_multiple_clip_depths = _plugin_context.get_host().get_supports_multiple_clip_depths()?;

                let supports_opencl = _plugin_context.get_host().get_opencl_render_supported().unwrap_or_default() == "true";
                let supports_opengl = _plugin_context.get_host().get_opengl_render_supported().unwrap_or_default() == "true";
                let supports_cuda   = _plugin_context.get_host().get_cuda_render_supported().unwrap_or_default() == "true";
                let supports_metal  = _plugin_context.get_host().get_metal_render_supported().unwrap_or_default() == "true";

                log::info!("Host supports OpenGL: {:?}", supports_opengl);
                log::info!("Host supports OpenCL: {:?}", supports_opencl);
                log::info!("Host supports CUDA: {:?}", supports_cuda);
                log::info!("Host supports Metal: {:?}", supports_metal);
                if !supports_opencl && !supports_opengl {
                    std::env::set_var("NO_OPENCL", "1");
                }

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

                if supports_opengl && !supports_opencl && !supports_cuda && !supports_metal {
                    // We'll initialize the devices in OpenGLContextAttached
                    let _ = effect_properties.set_opengl_render_supported("true");
                    return OK;
                }

                let opencl_devices = gyroflow_core::gpu::opencl::OclWrapper::list_devices();
                let wgpu_devices = gyroflow_core::gpu::wgpu::WgpuWrapper::list_devices();
                if !opencl_devices.is_empty() {
                    let _ = effect_properties.set_opencl_render_supported("true");
                    let _ = effect_properties.set_opengl_render_supported("true");
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
