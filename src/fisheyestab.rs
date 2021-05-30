use ofx::*;
use opencv::prelude::*;
use core::ffi::c_void;

plugin_module!(
	"nl.smslv.gyroflowofx.fisheyestab",
	ApiVersion(1),
	PluginVersion(1, 0),
	FisheyeStabilizerPlugin::new
);

#[derive(Default)]
struct FisheyeStabilizerPlugin {
}

impl FisheyeStabilizerPlugin {
	pub fn new() -> FisheyeStabilizerPlugin {
		FisheyeStabilizerPlugin::default()
	}
}
#[allow(unused)]
struct InstanceData {
	source_clip: ClipInstance,
	output_clip: ClipInstance,

	param_k: [[ParamHandle<Double>; 3]; 3],
	param_distortion: [ParamHandle<Double>; 4],
    param_calibration_dim: [ParamHandle<Double>; 2],
    param_correction_quat: [ParamHandle<Double>; 4],
}

struct PerFrameParams {
    camera_matrix: [[f64; 3]; 3],
    distortion_coeffs: [f64; 4],
    calibration_dim: [i32; 2],
    correction_quat: [f64; 4],
}

const PARAM_MAIN_NAME: &str = "Main";

const PARAM_K: &str = "K";

const PARAM_K_0_0: &str = "K00";
const PARAM_K_0_1: &str = "K01";
const PARAM_K_0_2: &str = "K02";
const PARAM_K_1_0: &str = "K10";
const PARAM_K_1_1: &str = "K11";
const PARAM_K_1_2: &str = "K12";
const PARAM_K_2_0: &str = "K20";
const PARAM_K_2_1: &str = "K21";
const PARAM_K_2_2: &str = "K22";

const PARAM_K_0_0_LABEL: &str = "K[0][0]";
const PARAM_K_0_1_LABEL: &str = "K[0][1]";
const PARAM_K_0_2_LABEL: &str = "K[0][2]";
const PARAM_K_1_0_LABEL: &str = "K[1][0]";
const PARAM_K_1_1_LABEL: &str = "K[1][1]";
const PARAM_K_1_2_LABEL: &str = "K[1][2]";
const PARAM_K_2_0_LABEL: &str = "K[2][0]";
const PARAM_K_2_1_LABEL: &str = "K[2][1]";
const PARAM_K_2_2_LABEL: &str = "K[2][2]";

const PARAM_DISTORTION: &str = "distortion";

const PARAM_DISTORTION_0: &str = "distortion0";
const PARAM_DISTORTION_1: &str = "distortion1";
const PARAM_DISTORTION_2: &str = "distortion2";
const PARAM_DISTORTION_3: &str = "distortion3";

const PARAM_DISTORTION_0_LABEL: &str = "Distortion 0";
const PARAM_DISTORTION_1_LABEL: &str = "Distortion 1";
const PARAM_DISTORTION_2_LABEL: &str = "Distortion 2";
const PARAM_DISTORTION_3_LABEL: &str = "Distortion 3";

const PARAM_CALIB_DIM: &str = "calDim";

const PARAM_CALIB_WIDTH: &str = "calWidth";
const PARAM_CALIB_HEIGHT: &str = "calHeight";

const PARAM_CALIB_WIDTH_LABEL: &str = "Calibration width";
const PARAM_CALIB_HEIGHT_LABEL: &str = "Calibration height";

const PARAM_CORRECTION_QUAT: &str = "correctionQuat";

const PARAM_CORRECTION_QUAT_W: &str = "corrW";
const PARAM_CORRECTION_QUAT_X: &str = "corrX";
const PARAM_CORRECTION_QUAT_Y: &str = "corrY";
const PARAM_CORRECTION_QUAT_Z: &str = "corrZ";

const PARAM_CORRECTION_QUAT_W_LABEL: &str = "Correction W";
const PARAM_CORRECTION_QUAT_X_LABEL: &str = "Correction X";
const PARAM_CORRECTION_QUAT_Y_LABEL: &str = "Correction Y";
const PARAM_CORRECTION_QUAT_Z_LABEL: &str = "Correction Z";

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

                let params = instance_data.get_per_frame_params(time)?;

                let src = source_image.get_descriptor::<RGBAColourF>()?;
                let dst = output_image.get_descriptor::<RGBAColourF>()?;

                let (dst_width, dst_height) = dst.data().dimensions();
                
                let img_dim = opencv::core::Size_::new(dst_width as i32, dst_height as i32);

                let scale = src.row(0).len() as f64 / params.calibration_dim[0] as f64;
                let mut scaled_k = Mat::from_slice_2d(&params.camera_matrix).unwrap();
                for r in 0..scaled_k.rows() {
                    for c in 0..scaled_k.cols() {
                        let e: &mut f64 = scaled_k.at_2d_mut(r, c).unwrap();
                        *e = *e * scale;
                    }
                }

                *scaled_k.at_2d_mut(2, 2).unwrap() = 1.0;

                let distortion_coeffs = Mat::from_slice(&params.distortion_coeffs).unwrap();

                let mut new_k = Mat::default();
                opencv::calib3d::estimate_new_camera_matrix_for_undistort_rectify(
                    &scaled_k, 
                    &distortion_coeffs, 
                    img_dim,
                    &Mat::eye(3, 3, opencv::core::CV_32F).unwrap(),
                    &mut new_k,
                    0.0,
                    img_dim,
                    1.1,
                ).unwrap();

                let q = cgmath::Quaternion::new(
                    -params.correction_quat[0],
                    params.correction_quat[1],
                    -params.correction_quat[2],
                    -params.correction_quat[3],
                );
                let qm = cgmath::Matrix3::from(q);
                let mut r = Mat::new_rows_cols_with_default(3, 3, opencv::core::CV_32F, Default::default()).unwrap();
                *r.at_2d_mut(0, 0).unwrap() = qm.x.x as f32;
                *r.at_2d_mut(0, 1).unwrap() = qm.y.x as f32;
                *r.at_2d_mut(0, 2).unwrap() = qm.z.x as f32;
                *r.at_2d_mut(1, 0).unwrap() = qm.x.y as f32;
                *r.at_2d_mut(1, 1).unwrap() = qm.y.y as f32;
                *r.at_2d_mut(1, 2).unwrap() = qm.z.y as f32;
                *r.at_2d_mut(2, 0).unwrap() = qm.x.z as f32;
                *r.at_2d_mut(2, 1).unwrap() = qm.y.z as f32;
                *r.at_2d_mut(2, 2).unwrap() = qm.z.z as f32;

                let mut map1 = Mat::default();
                let mut map2 = Mat::default();
                opencv::calib3d::fisheye_init_undistort_rectify_map(
                    &scaled_k, 
                    &distortion_coeffs,
                    &r,
                    &new_k,
                    img_dim,
                    opencv::core::CV_16SC2,
                    &mut map1,
                    &mut map2
                ).unwrap();

                let mut src_buf = src.data();
                let src_mat = unsafe {
                    Mat::new_rows_cols_with_data(
                        dst_height as i32,
                        dst.row(0).len() as i32,
                        opencv::core::CV_32FC4,
                        src_buf.ptr_mut(0) as *mut c_void,
                        (src_buf.byte_offset(0, 1) - src_buf.byte_offset(0, 0)) as usize).unwrap()
                };

                let mut dst_buf = dst.data();
                let mut dst_mat = unsafe {
                    Mat::new_rows_cols_with_data(
                        dst_height as i32,
                        dst.row(0).len() as i32,
                        opencv::core::CV_32FC4,
                        dst_buf.ptr_mut(0) as *mut c_void,
                        (dst_buf.byte_offset(0, 1) - dst_buf.byte_offset(0, 0)) as usize).unwrap()
                };

                opencv::imgproc::remap(&src_mat, &mut dst_mat, &map1, &map2, 
                    opencv::imgproc::INTER_LINEAR,
                    opencv::core::BORDER_CONSTANT,
                    Default::default()
                ).unwrap();

				if effect.abort()? {
					FAILED
				} else {
					OK
				}
			}

			CreateInstance(ref mut effect) => {
				let param_set = effect.parameter_set()?;

				let source_clip = effect.get_simple_input_clip()?;
				let output_clip = effect.get_output_clip()?;

                let param_k = [
                    [
                        param_set.parameter(PARAM_K_0_0)?, 
                        param_set.parameter(PARAM_K_0_1)?, 
                        param_set.parameter(PARAM_K_0_2)?,
                    ],
                    [
                        param_set.parameter(PARAM_K_1_0)?, 
                        param_set.parameter(PARAM_K_1_1)?, 
                        param_set.parameter(PARAM_K_1_2)?,
                    ],
                    [
                        param_set.parameter(PARAM_K_2_0)?, 
                        param_set.parameter(PARAM_K_2_1)?, 
                        param_set.parameter(PARAM_K_2_2)?,
                    ],
                ];

                let param_distortion = [
                    param_set.parameter(PARAM_DISTORTION_0)?,
                    param_set.parameter(PARAM_DISTORTION_1)?,
                    param_set.parameter(PARAM_DISTORTION_2)?,
                    param_set.parameter(PARAM_DISTORTION_3)?,
                ];

                let param_calibration_dim = [
                    param_set.parameter(PARAM_CALIB_WIDTH)?,
                    param_set.parameter(PARAM_CALIB_HEIGHT)?,
                ];

                let param_correction_quat = [
                    param_set.parameter(PARAM_CORRECTION_QUAT_W)?,
                    param_set.parameter(PARAM_CORRECTION_QUAT_X)?,
                    param_set.parameter(PARAM_CORRECTION_QUAT_Y)?,
                    param_set.parameter(PARAM_CORRECTION_QUAT_Z)?,
                ];

				effect.set_instance_data(InstanceData {
					source_clip,
					output_clip,
					param_k,
					param_distortion,
					param_calibration_dim,
                    param_correction_quat,
				})?;

				OK
			}

			DestroyInstance(ref mut _effect) => OK,

			DescribeInContext(ref mut effect, ref _in_args) => {
				let mut output_clip = effect.new_output_clip()?;
				output_clip
					.set_supported_components(&[ImageComponent::RGBA])?;
                
				let mut input_clip = effect.new_simple_input_clip()?;
				input_clip
					.set_supported_components(&[ImageComponent::RGBA])?;

				fn define_plain_param(
					param_set: &mut ParamSetHandle,
					name: &str,
                    default: f64,
					label: &'static str,
					parent: Option<&'static str>,
				) -> Result<()> {
					let mut param_props = param_set.param_define_double(name)?;

					param_props.set_double_type(ParamDoubleType::Plain)?;
					param_props.set_label(label)?;
					param_props.set_hint(label)?;
					param_props.set_default(default)?;
					param_props.set_display_min(-100.0)?;
					param_props.set_display_max(100.0)?;
					param_props.set_script_name(name)?;

					if let Some(parent) = parent {
						param_props.set_parent(parent)?;
					}

					Ok(())
				}

				let mut param_set = effect.parameter_set()?;

                let mut param_props = param_set.param_define_group(PARAM_K)?;
				param_props.set_hint("Camera matrix")?;
				param_props.set_label("Camera matrix")?;

				define_plain_param(&mut param_set,PARAM_K_0_0,2004.559898061336, PARAM_K_0_0_LABEL,Some(PARAM_K))?;
				define_plain_param(&mut param_set,PARAM_K_0_1,0.0, PARAM_K_0_1_LABEL,Some(PARAM_K))?;
				define_plain_param(&mut param_set,PARAM_K_0_2,1920.0, PARAM_K_0_2_LABEL,Some(PARAM_K))?;
				define_plain_param(&mut param_set,PARAM_K_1_0,0.0, PARAM_K_1_0_LABEL,Some(PARAM_K))?;
				define_plain_param(&mut param_set,PARAM_K_1_1,1502.6021031882099, PARAM_K_1_1_LABEL,Some(PARAM_K))?;
				define_plain_param(&mut param_set,PARAM_K_1_2,1080.0, PARAM_K_1_2_LABEL,Some(PARAM_K))?;
				define_plain_param(&mut param_set,PARAM_K_2_0,0.0, PARAM_K_2_0_LABEL,Some(PARAM_K))?;
				define_plain_param(&mut param_set,PARAM_K_2_1,0.0, PARAM_K_2_1_LABEL,Some(PARAM_K))?;
				define_plain_param(&mut param_set,PARAM_K_2_2,1.0, PARAM_K_2_2_LABEL,Some(PARAM_K))?;

                let mut param_props = param_set.param_define_group(PARAM_CALIB_DIM)?;
				param_props.set_hint("Camera calibration dimensions")?;
				param_props.set_label("Camera calibration dimensions")?;

                define_plain_param(&mut param_set,PARAM_CALIB_WIDTH,3840.0, PARAM_CALIB_WIDTH_LABEL,None)?;
                define_plain_param(&mut param_set,PARAM_CALIB_HEIGHT,2160.0, PARAM_CALIB_HEIGHT_LABEL,None)?;

                let mut param_props = param_set.param_define_group(PARAM_DISTORTION)?;
				param_props.set_hint("Distortion coefficients")?;
				param_props.set_label("Distortion coefficients")?;

				define_plain_param(&mut param_set,PARAM_DISTORTION_0,-0.04614696357651861, PARAM_DISTORTION_0_LABEL,Some(PARAM_DISTORTION))?;
				define_plain_param(&mut param_set,PARAM_DISTORTION_1,0.027871487382326275, PARAM_DISTORTION_1_LABEL,Some(PARAM_DISTORTION))?;
				define_plain_param(&mut param_set,PARAM_DISTORTION_2,-0.04499706001247255, PARAM_DISTORTION_2_LABEL,Some(PARAM_DISTORTION))?;
				define_plain_param(&mut param_set,PARAM_DISTORTION_3,0.017210690844729263, PARAM_DISTORTION_3_LABEL,Some(PARAM_DISTORTION))?;

                let mut param_props = param_set.param_define_group(PARAM_CORRECTION_QUAT)?;
				param_props.set_hint("Correction quaternion")?;
				param_props.set_label("Correction quaternion")?;

                define_plain_param(&mut param_set,PARAM_CORRECTION_QUAT_W,0.9999816018844726, PARAM_CORRECTION_QUAT_W_LABEL,Some(PARAM_CORRECTION_QUAT))?;
                define_plain_param(&mut param_set,PARAM_CORRECTION_QUAT_X,0.005914784980046915, PARAM_CORRECTION_QUAT_X_LABEL,Some(PARAM_CORRECTION_QUAT))?;
                define_plain_param(&mut param_set,PARAM_CORRECTION_QUAT_Y,0.0012299438397453124, PARAM_CORRECTION_QUAT_Y_LABEL,Some(PARAM_CORRECTION_QUAT))?;
                define_plain_param(&mut param_set,PARAM_CORRECTION_QUAT_Z,0.0005463051847160959, PARAM_CORRECTION_QUAT_Z_LABEL,Some(PARAM_CORRECTION_QUAT))?;

				param_set
					.param_define_page(PARAM_MAIN_NAME)?
					.set_children(&[
						PARAM_K_0_0,
						PARAM_K_0_1,
						PARAM_K_0_2,
						PARAM_K_1_0,
						PARAM_K_1_1,
						PARAM_K_1_2,
						PARAM_K_2_0,
						PARAM_K_2_1,
						PARAM_K_2_2,
						PARAM_CALIB_WIDTH,
						PARAM_CALIB_HEIGHT,
						PARAM_DISTORTION_0,
						PARAM_DISTORTION_1,
						PARAM_DISTORTION_2,
						PARAM_DISTORTION_3,
						PARAM_CORRECTION_QUAT_W,
						PARAM_CORRECTION_QUAT_X,
						PARAM_CORRECTION_QUAT_Y,
						PARAM_CORRECTION_QUAT_Z,
					])?;

				OK
			}

			Describe(ref mut effect) => {
				let mut effect_properties: EffectDescriptor = effect.properties()?;
				effect_properties.set_grouping("Warp")?;

				effect_properties.set_label("Fisheye stabilizer")?;
				effect_properties.set_short_label("Fisheye stabilizer")?;
				effect_properties.set_long_label("Fisheye stabilizer")?;

				effect_properties.set_supported_pixel_depths(&[
					BitDepth::Float,
				])?;
				effect_properties.set_supported_contexts(&[
					ImageEffectContext::Filter,
				])?;

				OK
			}

            Load => {
                let opencl_have = opencv::core::have_opencl().unwrap();
                if opencl_have {
                    opencv::core::set_use_opencl(true).unwrap();
                    let mut platforms = opencv::types::VectorOfPlatformInfo::new();
                    opencv::core::get_platfoms_info(&mut platforms).unwrap();
                    for (platf_num, platform) in platforms.into_iter().enumerate() {
                        println!("Platform #{}: {}", platf_num, platform.name().unwrap());
                        for dev_num in 0..platform.device_number().unwrap() {
                            let mut dev = opencv::core::Device::default();
                            platform.get_device(&mut dev, dev_num).unwrap();
                            println!("  OpenCL device #{}: {}", dev_num, dev.name().unwrap());
                            println!("    vendor:  {}", dev.vendor_name().unwrap());
                            println!("    version: {}", dev.version().unwrap());
                        }
                    }
                }
                let opencl_use = opencv::core::use_opencl().unwrap();
                println!(
                    "OpenCL is {} and {}",
                    if opencl_have { "available" } else { "not available" },
                    if opencl_use { "enabled" } else { "disabled" },
                );

                OK
            }

            _ => REPLY_DEFAULT,
		}
	}
}

impl InstanceData {
	fn get_per_frame_params(&self, time: Time) -> Result<PerFrameParams> {
		let camera_matrix = [
            [
                self.param_k[0][0].get_value_at_time(time)?,
                self.param_k[0][1].get_value_at_time(time)?,
                self.param_k[0][2].get_value_at_time(time)?,
            ],
            [
                self.param_k[1][0].get_value_at_time(time)?,
                self.param_k[1][1].get_value_at_time(time)?,
                self.param_k[1][2].get_value_at_time(time)?,
            ],
            [
                self.param_k[2][0].get_value_at_time(time)?,
                self.param_k[2][1].get_value_at_time(time)?,
                self.param_k[2][2].get_value_at_time(time)?,
            ],
        ];

        let distortion_coeffs = [
            self.param_distortion[0].get_value_at_time(time)?,
            self.param_distortion[1].get_value_at_time(time)?,
            self.param_distortion[2].get_value_at_time(time)?,
            self.param_distortion[3].get_value_at_time(time)?,
        ];

        let calibration_dim = [
            self.param_calibration_dim[0].get_value_at_time(time)? as i32,
            self.param_calibration_dim[1].get_value_at_time(time)? as i32,
        ];

        let correction_quat = [
            self.param_correction_quat[0].get_value_at_time(time)?,
            self.param_correction_quat[1].get_value_at_time(time)?,
            self.param_correction_quat[2].get_value_at_time(time)?,
            self.param_correction_quat[3].get_value_at_time(time)?,
        ];

        Ok(PerFrameParams {
            camera_matrix,
            distortion_coeffs,
            calibration_dim,
            correction_quat,
        })
	}
}
