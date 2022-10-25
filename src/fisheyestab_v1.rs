use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gyroflow_core::{StabilizationManager, stabilization::RGBAf};
use gyroflow_core::gpu::{ BufferDescription, BufferSource };
use lru::LruCache;
use measure_time::*;
use ofx::*;

plugin_module!(
    "nl.smslv.gyroflowofx.fisheyestab_v1",
    ApiVersion(1),
    PluginVersion(1, 2),
    FisheyeStabilizerPlugin::new
);

#[derive(Default)]
struct FisheyeStabilizerPlugin {}

impl FisheyeStabilizerPlugin {
    pub fn new() -> FisheyeStabilizerPlugin {
        FisheyeStabilizerPlugin::default()
    }
}
#[allow(unused)]
struct InstanceData {
    source_clip: ClipInstance,
    output_clip: ClipInstance,

    param_gyrodata: ParamHandle<String>,
    param_fov: ParamHandle<Double>,
    param_smoothness: ParamHandle<Double>,
    param_lens_correction_strength: ParamHandle<Double>,
    param_status: ParamHandle<Bool>,
    gyrodata: LruCache<String, Arc<StabilizationManager<RGBAf>>>,
}

impl InstanceData {
    fn gyrodata(
        &mut self,
        width: usize,
        height: usize
    ) -> Result<Arc<StabilizationManager<RGBAf>>> {
        let gyrodata_filename = self.param_gyrodata.get_value()?;
        let key = format!("{gyrodata_filename}{width}{height}");
        let gyrodata = if let Some(gyrodata) = self.gyrodata.get(&key) {
            gyrodata.clone()
        } else {
            let gyrodata = StabilizationManager::default();
            gyrodata.import_gyroflow_file(&gyrodata_filename, true, |_|(), Arc::new(AtomicBool::new(false))).map_err(|e| {
                error!("load_gyro_data error: {}", &e);
                Error::UnknownError
            })?;

            let video_size = {
                let mut params = gyrodata.params.write();
                params.framebuffer_inverted = true;
                params.video_size
            };

            let org_ratio = video_size.0 as f64 / video_size.1 as f64;

            let src_rect = Self::get_center_rect(width, height, org_ratio);
            gyrodata.set_size(src_rect.2, src_rect.3);
            gyrodata.set_output_size(width, height);

            {
                let mut stab = gyrodata.stabilization.write();
                stab.interpolation = gyroflow_core::stabilization::Interpolation::Lanczos4;
            }

            gyrodata.invalidate_smoothing();
            gyrodata.recompute_blocking();
            gyrodata.params.write().calculate_ramped_timestamps(&gyrodata.keyframes.read());

            self.gyrodata
                .put(key.to_owned(), Arc::new(gyrodata));
            self.gyrodata
                .get(&key)
                .map(Arc::clone)
                .ok_or(Error::UnknownError)?
        };

        Ok(gyrodata)
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

struct PerFrameParams {}

const PARAM_MAIN_NAME: &str = "Main";

const PARAM_GYRODATA: &str = "gyrodata";

const PARAM_GYRODATA_LABEL: &str = "Gyroflow file";

impl Execute for FisheyeStabilizerPlugin {
    #[allow(clippy::float_cmp)]
    fn execute(&mut self, _plugin_context: &PluginContext, action: &mut Action) -> Result<Int> {
        use Action::*;

        match *action {
            Render(ref mut effect, ref in_args) => {
                let time = in_args.get_time()?;
                let instance_data: &mut InstanceData = effect.get_instance_data()?;
                let source_rect: RectD = instance_data.source_clip.get_region_of_definition(time)?;
                let output_rect: RectD = instance_data.output_clip.get_region_of_definition(time)?;

                let mut source_image = instance_data.source_clip.get_image(time)?;
                let output_image = instance_data.output_clip.get_image_mut(time)?;
                let output_image = output_image.borrow_mut();

                let fov = instance_data.param_fov.get_value_at_time(time)?;
                let lens_correction_strength = instance_data.param_lens_correction_strength.get_value_at_time(time)? / 100.0;
                let smoothness = instance_data.param_smoothness.get_value_at_time(time)?;

                let mut width = source_rect.x2 as usize;
                let mut height = source_rect.y2 as usize;
                if let Ok(srcb) = source_image.get_descriptor::<RGBAColourF>() {
                    let dims = srcb.data().dimensions();
                    width = dims.0 as usize;
                    height = dims.1 as usize;
                }

                let stab = instance_data.gyrodata(width, height)?;

                let params = stab.params.read();
                let params_fov = params.fov;
                let params_lens_correction_strength = params.lens_correction_amount;
                let params_smoothness = stab.smoothing.read().current().get_parameter("smoothness");
                let src_fps = instance_data.source_clip.get_frame_rate().unwrap_or(0.0);
                let fps = params.fps;
                let org_ratio = params.video_size.0 as f64 / params.video_size.1 as f64;
                let src_rect = InstanceData::get_center_rect(width, height, org_ratio);

                let loaded = params.duration_ms > 0.0;

                instance_data.param_fov.set_enabled(loaded)?;
                instance_data.param_smoothness.set_enabled(loaded)?;
                instance_data.param_lens_correction_strength.set_enabled(loaded)?;

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
                } else {
                    instance_data.param_status.set_label("OK")?;
                    instance_data.param_status.set_hint("OK")?;
                }

                speed_stretch *= src_fps / fps;
                log::debug!("Source file fps: {src_fps}");
                log::debug!("Params fps: {fps}");
                log::debug!("Speed stretch: {speed_stretch}");

                let mut timestamp_us = ((time / src_fps * 1_000_000.0) * speed_stretch).round() as i64;

                let source_timestamp_us = params.get_source_timestamp_at_ramped_timestamp(timestamp_us);
                if source_timestamp_us != timestamp_us {
                    let new_time = (source_timestamp_us as f64 / speed_stretch / 1_000_000.0 * src_fps).round();
                    source_image = instance_data.source_clip.get_image(new_time)?;
                    timestamp_us = ((new_time / src_fps * 1_000_000.0) * speed_stretch).round() as i64;
                }
                drop(params);

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

                let processed =
                    if in_args.get_opencl_enabled()? {
                        use std::ffi::c_void;
                        let src_stride = source_rect.x2 as usize * 4 * std::mem::size_of::<f32>();
                        let out_stride = output_rect.x2 as usize * 4 * std::mem::size_of::<f32>();

                        stab.process_pixels(timestamp_us, &mut BufferDescription {
                            input_size:  (source_rect.x2 as usize, source_rect.y2 as usize, src_stride),
                            output_size: (output_rect.x2 as usize, output_rect.y2 as usize, out_stride),
                            input_rect: Some(src_rect),
                            output_rect: None,
                            buffers: BufferSource::OpenCL {
                                input: source_image.get_data()? as *mut c_void,
                                output: output_image.get_data()? as *mut c_void,
                                queue: in_args.get_opencl_command_queue()? as *mut c_void,
                            }
                        }).is_some()
                } else if in_args.get_cuda_enabled()? {
                    false
                } else {
                    let src = source_image.get_descriptor::<RGBAColourF>()?;
                    let dst = output_image.get_descriptor::<RGBAColourF>()?;

                    let mut src_buf = src.data();
                    let mut dst_buf = dst.data();
                    let src_stride = src_buf.stride_bytes().abs() as usize;
                    let out_stride = dst_buf.stride_bytes().abs() as usize;

                    let out = stab.process_pixels(timestamp_us, &mut BufferDescription {
                        input_size:  (src_buf.dimensions().0 as usize, src_buf.dimensions().1 as usize, src_stride),
                        output_size: (dst_buf.dimensions().0 as usize, dst_buf.dimensions().1 as usize, out_stride),
                        input_rect: Some(src_rect),
                        output_rect: None,
                        buffers: BufferSource::Cpu {
                            input:  unsafe { std::slice::from_raw_parts_mut(src_buf.ptr_mut(0), src_buf.bytes()) },
                            output: unsafe { std::slice::from_raw_parts_mut(dst_buf.ptr_mut(0), dst_buf.bytes()) }
                        }
                    });
                    out.is_some()
                };

                if effect.abort()? || !processed {
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

                let param_gyrodata = param_set.parameter(PARAM_GYRODATA)?;

                let param_fov = param_set.parameter("FOV")?;
                let param_smoothness = param_set.parameter("Smoothness")?;
                let param_lens_correction_strength = param_set.parameter("LensCorrectionStrength")?;
                let param_status = param_set.parameter("Status")?;

                effect.set_instance_data(InstanceData {
                    source_clip,
                    output_clip,
                    param_gyrodata,
                    param_smoothness,
                    param_lens_correction_strength,
                    param_fov,
                    param_status,
                    gyrodata: LruCache::new(std::num::NonZeroUsize::new(1).unwrap()),
                })?;

                OK
            }
            InstanceChanged(ref mut effect, ref mut in_args) => {
                if in_args.get_name()? == "gyrodata" && in_args.get_change_reason()? == Change::UserEdited {
                    let instance_data: &mut InstanceData = effect.get_instance_data()?;
                    let stab = instance_data.gyrodata(16, 16)?;
                    let params = stab.params.read();
                    let loaded = params.duration_ms > 0.0;

                    if loaded {
                        let smoothness = stab.smoothing.read().current().get_parameter("smoothness");

                        instance_data.param_fov.set_value(params.fov)?;
                        instance_data.param_smoothness.set_value(smoothness)?;
                        instance_data.param_lens_correction_strength.set_value((params.lens_correction_amount * 100.0).min(100.0))?;
                    }
                }

                OK
            }

            DestroyInstance(ref mut _effect) => OK,

            DescribeInContext(ref mut effect, ref _in_args) => {
                let mut output_clip = effect.new_output_clip()?;
                output_clip.set_supported_components(&[ImageComponent::RGBA])?;

                let mut input_clip = effect.new_simple_input_clip()?;
                input_clip.set_supported_components(&[ImageComponent::RGBA])?;

                let mut param_set = effect.parameter_set()?;

                let mut param_props = param_set.param_define_string(PARAM_GYRODATA)?;
                param_props.set_string_type(ParamStringType::FilePath)?;
                param_props.set_label(PARAM_GYRODATA_LABEL)?;
                param_props.set_hint(PARAM_GYRODATA_LABEL)?;
                param_props.set_script_name(PARAM_GYRODATA)?;

                let mut param_fov = param_set.param_define_double("FOV")?;
                param_fov.set_double_type(ParamDoubleType::Plain)?;
                param_fov.set_default(1.0)?;
                param_fov.set_display_min(0.1)?;
                param_fov.set_display_max(3.0)?;
                param_fov.set_label("FOV")?;
                param_fov.set_hint("FOV")?;
                param_fov.set_script_name("FOV")?;
                param_fov.set_enabled(false)?;

                let mut params_smoothness = param_set.param_define_double("Smoothness")?;
                params_smoothness.set_double_type(ParamDoubleType::Plain)?;
                params_smoothness.set_default(0.5)?;
                params_smoothness.set_display_min(0.01)?;
                params_smoothness.set_display_max(3.0)?;
                params_smoothness.set_label("Smoothness")?;
                params_smoothness.set_hint("Smoothness")?;
                params_smoothness.set_script_name("Smoothness")?;
                params_smoothness.set_enabled(false)?;

                let mut param_lens_correction = param_set.param_define_double("LensCorrectionStrength")?;
                param_lens_correction.set_double_type(ParamDoubleType::Plain)?;
                param_lens_correction.set_default(100.0)?;
                param_lens_correction.set_display_min(0.0)?;
                param_lens_correction.set_display_max(100.0)?;
                param_lens_correction.set_label("Lens correction")?;
                param_lens_correction.set_hint("Lens correction")?;
                param_lens_correction.set_script_name("LensCorrectionStrength")?;
                param_lens_correction.set_enabled(false)?;

                let mut param_status = param_set.param_define_boolean("Status")?;
                param_status.set_label("Status")?;
                param_status.set_hint("Status")?;
                param_status.set_enabled(false)?;

                // if let Some(parent) = None {
                //     param_props.set_parent(parent)?;
                //     param_video_speed.set_parent(parent)?;
                //     param_fov.set_parent(parent)?;
                // }

                param_set
                    .param_define_page(PARAM_MAIN_NAME)?
                    .set_children(&[PARAM_GYRODATA, "FOV", "Smoothness", "LensCorrectionStrength", "Status"])?;

                OK
            }

            Describe(ref mut effect) => {
                // d!("host supports opencl: {:?}", _plugin_context.get_host().get_opencl_render_supported());
                // d!("host supports cuda: {:?}", _plugin_context.get_host().get_cuda_render_supported());
                // d!("host supports metal: {:?}", _plugin_context.get_host().get_metal_render_supported());

                let mut effect_properties: EffectDescriptor = effect.properties()?;
                effect_properties.set_grouping("Warp")?;

                effect_properties.set_label("Gyroflow")?;
                effect_properties.set_short_label("Gyroflow")?;
                effect_properties.set_long_label("Gyroflow")?;

                // effect_properties.set_supported_pixel_depths(&[BitDepth::Byte, BitDepth::Short, BitDepth::Float])?;
                effect_properties.set_supported_pixel_depths(&[BitDepth::Float])?;
                effect_properties.set_supported_contexts(&[ImageEffectContext::Filter])?;
                effect_properties.set_supports_tiles(false)?;

                effect_properties.set_single_instance(false)?;
                effect_properties.set_host_frame_threading(false)?;
                effect_properties.set_render_thread_safety(ImageEffectRender::FullySafe)?;
                effect_properties.set_supports_multi_resolution(true)?;
                effect_properties.set_temporal_clip_access(true)?;
                effect_properties.set_opencl_render_supported("true")?;
                // effect_properties.set_cuda_render_supported("true")?;
                // effect_properties.set_metal_render_supported("true")?;

                OK
            }

            Load => OK,

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
