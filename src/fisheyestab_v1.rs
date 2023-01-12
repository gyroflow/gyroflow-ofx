use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gyroflow_core::{StabilizationManager, stabilization::RGBAf};
use gyroflow_core::gpu::{ BufferDescription, Buffers, BufferSource };
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
    param_desqueezed: ParamHandle<Bool>,
    param_status: ParamHandle<Bool>,
    gyrodata: LruCache<String, Arc<StabilizationManager<RGBAf>>>,
}

impl InstanceData {
    fn gyrodata(
        &mut self,
        width: usize,
        height: usize,
        desqueezed: bool,
        additional_key: i32
    ) -> Result<Arc<StabilizationManager<RGBAf>>> {
        let gyrodata_filename = self.param_gyrodata.get_value()?;
        let key = format!("{gyrodata_filename}{width}{height}{desqueezed}{additional_key}");
        let gyrodata = if let Some(gyrodata) = self.gyrodata.get(&key) {
            gyrodata.clone()
        } else {
            let gyrodata = StabilizationManager::default();
            gyrodata.import_gyroflow_file(&gyrodata_filename, true, |_|(), Arc::new(AtomicBool::new(false))).map_err(|e| {
                error!("load_gyro_data error: {}", &e);
                Error::UnknownError
            })?;

            if desqueezed {
                let (x_stretch, y_stretch) = {
                    let lens = gyrodata.lens.read();
                    (lens.input_horizontal_stretch, lens.input_vertical_stretch)
                };
                if (x_stretch > 0.01 && x_stretch != 1.0) || (y_stretch > 0.01 && y_stretch != 1.0) {
                    {
                        let mut params = gyrodata.params.write();
                        params.video_size.0 = (params.video_size.0 as f64 * x_stretch).round() as usize;
                        params.video_size.1 = (params.video_size.1 as f64 * y_stretch).round() as usize;
                    }
                    {
                        let mut lens = gyrodata.lens.write();
                        lens.input_horizontal_stretch = 1.0;
                        lens.input_vertical_stretch = 1.0;
                    }
                }
            }
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

impl Execute for FisheyeStabilizerPlugin {
    #[allow(clippy::float_cmp)]
    fn execute(&mut self, _plugin_context: &PluginContext, action: &mut Action) -> Result<Int> {
        use Action::*;

        match *action {
            Render(ref mut effect, ref in_args) => {
                let _time = std::time::Instant::now();

                let mut additional_key = -1;
                #[cfg(any(target_os = "windows", target_os = "linux"))]
                if in_args.get_cuda_enabled().unwrap_or_default() {
                    additional_key = gyroflow_core::gpu::wgpu_interop_cuda::get_current_cuda_device();
                }

                let time = in_args.get_time()?;
                let instance_data: &mut InstanceData = effect.get_instance_data()?;

                let output_image = instance_data.output_clip.get_image_mut(time)?;
                let output_image = output_image.borrow_mut();

                let fov = instance_data.param_fov.get_value_at_time(time)?;
                let lens_correction_strength = instance_data.param_lens_correction_strength.get_value_at_time(time)? / 100.0;
                let smoothness = instance_data.param_smoothness.get_value_at_time(time)?;
                let desqueezed = instance_data.param_desqueezed.get_value_at_time(time)?;

                let output_rect: RectI = output_image.get_region_of_definition()?;
                let out_width= (output_rect.x2 - output_rect.x1) as usize;
                let out_height= (output_rect.y2 - output_rect.y1) as usize;

                let stab = instance_data.gyrodata(out_width, out_height, desqueezed, additional_key)?;

                let params = stab.params.read();
                let params_fov = params.fov;
                let params_lens_correction_strength = params.lens_correction_amount;
                let params_smoothness = stab.smoothing.read().current().get_parameter("smoothness");
                let fps = params.fps;
                let src_fps = instance_data.source_clip.get_frame_rate().unwrap_or(fps);
                let org_ratio = params.video_size.0 as f64 / params.video_size.1 as f64;

                let loaded = params.duration_ms > 0.0;

                instance_data.param_fov.set_enabled(loaded)?;
                instance_data.param_smoothness.set_enabled(loaded)?;
                instance_data.param_lens_correction_strength.set_enabled(loaded)?;
                instance_data.param_desqueezed.set_enabled(loaded)?;

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

                let src_stride = (source_rect.x2 - source_rect.x1) as usize * 4 * std::mem::size_of::<f32>();
                let out_stride = (output_rect.x2 - output_rect.x1) as usize * 4 * std::mem::size_of::<f32>();
                let src_size = ((source_rect.x2 - source_rect.x1) as usize, (source_rect.y2 - source_rect.y1) as usize, src_stride);
                let out_size = ((output_rect.x2 - output_rect.x1) as usize, (output_rect.y2 - output_rect.y1) as usize, out_stride);

                let src_rect = InstanceData::get_center_rect(out_width, out_height, org_ratio);

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
                    if in_args.get_opencl_enabled().unwrap_or_default() {
                        use std::ffi::c_void;
                        let queue = in_args.get_opencl_command_queue()? as *mut c_void;

                        stab.process_pixels(timestamp_us, &mut Buffers {
                            input: BufferDescription {
                                size: src_size,
                                rect: Some(src_rect),
                                data: BufferSource::OpenCL { texture: source_image.get_data()? as *mut c_void, queue },
                                texture_copy: false
                            },
                            output: BufferDescription {
                                size: out_size,
                                rect: None,
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

                        stab.process_pixels(timestamp_us, &mut Buffers {
                            input: BufferDescription {
                                size: src_size,
                                rect: Some(src_rect),
                                data: BufferSource::MetalBuffer { buffer: in_ptr, command_queue },
                                texture_copy: false
                            },
                            output: BufferDescription {
                                size: out_size,
                                rect: None,
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

                        let ret = stab.process_pixels(timestamp_us, &mut Buffers {
                            input: BufferDescription {
                                size: src_size,
                                rect: Some(src_rect),
                                data: BufferSource::CUDABuffer { buffer: in_ptr },
                                texture_copy: true
                            },
                            output: BufferDescription {
                                size: out_size,
                                rect: None,
                                data: BufferSource::CUDABuffer { buffer: out_ptr },
                                texture_copy: true
                            }
                        });

                        ret
                    }
                } else {
                    let src = source_image.get_descriptor::<RGBAColourF>()?;
                    let dst = output_image.get_descriptor::<RGBAColourF>()?;

                    let mut src_buf = src.data();
                    let mut dst_buf = dst.data();

                    stab.process_pixels(timestamp_us, &mut Buffers {
                        input: BufferDescription {
                            size: src_size,
                            rect: Some(src_rect),
                            data: BufferSource::Cpu { buffer: unsafe { std::slice::from_raw_parts_mut(src_buf.ptr_mut(0), src_buf.bytes()) } },
                            texture_copy: false
                        },
                        output: BufferDescription {
                            size: out_size,
                            rect: None,
                            data: BufferSource::Cpu { buffer: unsafe { std::slice::from_raw_parts_mut(dst_buf.ptr_mut(0), dst_buf.bytes()) } },
                            texture_copy: false
                        }
                    })
                };

                // log::info!("Rendered | {additional_key} | {}x{} in {:.2}ms: {:?}", src_size.0, src_size.1, _time.elapsed().as_micros() as f64 / 1000.0, processed);

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

                let param_gyrodata = param_set.parameter("gyrodata")?;

                let param_fov = param_set.parameter("FOV")?;
                let param_smoothness = param_set.parameter("Smoothness")?;
                let param_lens_correction_strength = param_set.parameter("LensCorrectionStrength")?;
                let param_desqueezed = param_set.parameter("Desqueezed")?;
                let param_status = param_set.parameter("Status")?;

                effect.set_instance_data(InstanceData {
                    source_clip,
                    output_clip,
                    param_gyrodata,
                    param_smoothness,
                    param_lens_correction_strength,
                    param_fov,
                    param_desqueezed,
                    param_status,
                    gyrodata: LruCache::new(std::num::NonZeroUsize::new(8).unwrap())
                })?;

                OK
            }
            InstanceChanged(ref mut effect, ref mut in_args) => {
                if in_args.get_name()? == "gyrodata" && in_args.get_change_reason()? == Change::UserEdited {
                    let instance_data: &mut InstanceData = effect.get_instance_data()?;
                    let stab = instance_data.gyrodata(16, 16, false, 0)?;
                    let params = stab.params.read();
                    let loaded = params.duration_ms > 0.0;

                    if loaded {
                        let smoothness = stab.smoothing.read().current().get_parameter("smoothness");

                        instance_data.param_fov.set_value(params.fov)?;
                        instance_data.param_smoothness.set_value(smoothness)?;
                        instance_data.param_lens_correction_strength.set_value((params.lens_correction_amount * 100.0).min(100.0))?;
                    }
                    instance_data.gyrodata.clear();
                }

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

                let mut param_props = param_set.param_define_string("gyrodata")?;
                param_props.set_string_type(ParamStringType::FilePath)?;
                param_props.set_label("Gyroflow file")?;
                param_props.set_hint("Gyroflow file")?;
                param_props.set_script_name("gyrodata")?;

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

                let mut param_desqueezed = param_set.param_define_boolean("Desqueezed")?;
                param_desqueezed.set_label("Disable Gyroflow's stretch")?;
                param_desqueezed.set_hint("If you used Input stretch in the lens profile in Gyroflow, and you de-stretched the video separately in Resolve, check this to disable Gyroflow's internal stretching.")?;
                param_desqueezed.set_enabled(false)?;

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
                    .param_define_page("Main")?
                    .set_children(&["gyrodata", "FOV", "Smoothness", "LensCorrectionStrength", "Status", "Desqueezed"])?;

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

                // effect_properties.set_supported_pixel_depths(&[BitDepth::Byte, BitDepth::Short, BitDepth::Float])?;
                effect_properties.set_supported_pixel_depths(&[BitDepth::Float])?;
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
                    effect_properties.set_opencl_render_supported("true")?;
                }

                let _has_metal  = wgpu_devices.iter().any(|x| x.contains("(Metal)"));
                let _has_vulkan = wgpu_devices.iter().any(|x| x.contains("(Vulkan)"));
                let _has_dx12   = wgpu_devices.iter().any(|x| x.contains("(Dx12)"));

                #[cfg(any(target_os = "macos", target_os = "ios"))]
                if _has_metal { effect_properties.set_metal_render_supported("true")?; }
                #[cfg(any(target_os = "windows", target_os = "linux"))]
                if _has_vulkan || _has_dx12 { effect_properties.set_cuda_render_supported("true")?; }

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
