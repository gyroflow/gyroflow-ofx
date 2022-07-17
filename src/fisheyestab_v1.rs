use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gyroflow_core::{StabilizationManager, stabilization::RGBAf};
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
    gyrodata: LruCache<String, Arc<StabilizationManager<RGBAf>>>,
}

impl InstanceData {
    fn gyrodata(
        &mut self,
        width: usize,
        height: usize,
        stride: usize,
    ) -> Result<Arc<StabilizationManager<RGBAf>>> {
        let gyrodata_filename = self.param_gyrodata.get_value()?;
        let key = format!("{gyrodata_filename}{width}{height}{stride}");
        let gyrodata = if let Some(gyrodata) = self.gyrodata.get(&key) {
            gyrodata.clone()
        } else {
            let gyrodata = StabilizationManager::default();
            gyrodata.import_gyroflow_file(&gyrodata_filename, true, |_|(), Arc::new(AtomicBool::new(false))).map_err(|e| {
                error!("load_gyro_data error: {}", &e);
                Error::UnknownError
            })?;
            gyrodata.params.write().framebuffer_inverted = true;

            let (video_size, bg) = {
                let params = gyrodata.params.read();
                (params.video_size, params.background)
            };

            let org_ratio = video_size.0 as f64 / video_size.1 as f64;

            gyrodata.set_size(width, height);
            gyrodata.set_output_size(width, height);

            let src_rect = Self::get_center_rect(width, height, org_ratio);
            gyrodata.params.write().size = (src_rect.2, src_rect.3);
            gyrodata.stabilization.write().init_size(bg, (src_rect.2, src_rect.3, stride), (width, height, stride));

            gyrodata.invalidate_smoothing();
            gyrodata.invalidate_zooming();
            gyrodata.recompute_blocking();

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
        if ((width as f64 / height as f64) - org_ratio).abs() > 0.1 {
            // Get center rect of original aspect ratio
            let rect = if width > height {
                ((width as f64 / org_ratio).round() as usize, height)
            } else {
                (width, (height as f64 / org_ratio).round() as usize)
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

                let source_image = instance_data.source_clip.get_image(time)?;
                let output_image = instance_data.output_clip.get_image_mut(time)?;
                let output_image = output_image.borrow_mut();

                let src = source_image.get_descriptor::<RGBAColourF>()?;
                let dst = output_image.get_descriptor::<RGBAColourF>()?;

                let mut src_buf = src.data();
                let mut dst_buf = dst.data();

                let processed = {
                    let width = src_buf.dimensions().0 as usize;
                    let height = src_buf.dimensions().1 as usize;

                    let stab = instance_data.gyrodata(
                        width,
                        height,
                        src_buf.stride_bytes().abs() as usize
                    )?;
                    let stab_params = stab.params.read();
                    let fps = stab_params.fps;
                    let timestamp_us = (time / fps * 1_000_000.0) as i64;

                    let org_ratio = stab_params.video_size.0 as f64 / stab_params.video_size.1 as f64;

                    let src_rect = InstanceData::get_center_rect(width, height, org_ratio);

                    drop(stab_params);

                    let src_offset = src_rect.1 as isize * src_buf.stride_bytes() + src_rect.0 as isize * 4 * 4;

                    stab.process_pixels(
                        timestamp_us,
                        (
                            src_rect.2,
                            src_rect.3,
                            src_buf.stride_bytes().abs() as usize
                        ),
                        (
                            dst_buf.dimensions().0 as usize,
                            dst_buf.dimensions().1 as usize,
                            dst_buf.stride_bytes().abs() as usize
                        ),
                        unsafe {
                            // TODO: length needs to subtract src_offset, but this fails in processing kernel to verify buffer size (because stride > width)
                            std::slice::from_raw_parts_mut(src_buf.ptr_mut(src_offset), src_buf.bytes())
                        },
                        unsafe {
                            std::slice::from_raw_parts_mut(dst_buf.ptr_mut(0), dst_buf.bytes())
                        },
                    )
                };

                if effect.abort()? || !processed {
                    FAILED
                } else {
                    OK
                }
            }

            CreateInstance(ref mut effect) => {
                let param_set = effect.parameter_set()?;

                let source_clip = effect.get_simple_input_clip()?;
                let output_clip = effect.get_output_clip()?;

                let param_gyrodata = param_set.parameter(PARAM_GYRODATA)?;

                effect.set_instance_data(InstanceData {
                    source_clip,
                    output_clip,
                    param_gyrodata,
                    gyrodata: LruCache::new(1),
                })?;

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

                if let Some(parent) = None {
                    param_props.set_parent(parent)?;
                }

                param_set
                    .param_define_page(PARAM_MAIN_NAME)?
                    .set_children(&[PARAM_GYRODATA])?;

                OK
            }

            Describe(ref mut effect) => {
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
