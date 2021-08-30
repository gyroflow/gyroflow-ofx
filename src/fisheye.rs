use std::f64::consts::FRAC_PI_2;

use float_ord::FloatOrd;
use itertools::Itertools;
use nalgebra::{Matrix3, Vector2, Vector4};
use ndarray::parallel::prelude::*;
use ndarray::prelude::*;
use rayon::iter::{IndexedParallelIterator, ParallelIterator};

pub fn undistort_rectify(
    k_m: &Matrix3<f64>,
    distortion_coeffs: &Vector4<f64>,
    r: &Matrix3<f64>,
    p: &Matrix3<f64>,
    src: &ArrayView3<f32>,
    dst: &mut ArrayViewMut3<f32>,
) {
    let size = src.dim();
    assert_eq!(dst.dim(), size);

    let size = { [size.0 as usize, size.1 as usize] };

    let f = Vector2::<f64>::new(k_m[(0, 0)], k_m[(1, 1)]);
    let c = Vector2::<f64>::new(k_m[(0, 2)], k_m[(1, 2)]);
    let i_r = (p * r).pseudo_inverse(0.000001).unwrap();

    dst.axis_chunks_iter_mut(Axis(0), 1)
        .into_par_iter()
        .enumerate()
        .for_each(|(dst_y, mut dst)| {
            let mut x = dst_y as f64 * i_r[(0, 1)] + i_r[(0, 2)];
            let mut y = dst_y as f64 * i_r[(1, 1)] + i_r[(1, 2)];
            let mut w = dst_y as f64 * i_r[(2, 1)] + i_r[(2, 2)];

            for dst_x in 0..size[1] {
                let (u, v) = if w <= 0.0 {
                    let u = if x > 0.0 {
                        f64::NEG_INFINITY
                    } else {
                        f64::INFINITY
                    };
                    let v = if y > 0.0 {
                        f64::NEG_INFINITY
                    } else {
                        f64::INFINITY
                    };
                    (u, v)
                } else {
                    let x = x / w;
                    let y = y / w;

                    let r = (x * x + y * y).sqrt();
                    let theta = r.atan();
                    let theta_d = theta * (1.0 + distortion_coeffs.dot(&even_powers_4(theta)));

                    let scale = if r == 0.0 { 1.0 } else { theta_d / r };

                    let u = f[0] * x * scale + c[0];
                    let v = f[1] * y * scale + c[1];

                    (u, v)
                };

                let u_fract = u.fract() as f32;
                let v_fract = v.fract() as f32;

                let iu_min = u.floor() as i32;
                let iv_min = v.floor() as i32;
                let iu_max = iu_min + 1;
                let iv_max = iv_min + 1;

                let mut dst_pixel = dst.slice_mut(s![0, dst_x, ..]);

                if iu_min >= 0
                    && iu_min < size[1] as i32
                    && iu_max >= 0
                    && iu_max < size[1] as i32
                    && iv_min >= 0
                    && iv_min < size[0] as i32
                    && iv_max >= 0
                    && iv_max < size[0] as i32
                {
                    // common case
                    let left_coeff = 1.0 - u_fract;
                    let right_coeff = u_fract;

                    let top_coeff = 1.0 - v_fract;
                    let bottom_coeff = v_fract;

                    let left_top = src
                        .slice(s![iv_min, iu_min, ..])
                        .map(|v| v * left_coeff * top_coeff);
                    let left_bottom = src
                        .slice(s![iv_min, iu_max, ..])
                        .map(|v| v * left_coeff * bottom_coeff);
                    let right_top = src
                        .slice(s![iv_max, iu_min, ..])
                        .map(|v| v * right_coeff * top_coeff);
                    let right_bottom = src
                        .slice(s![iv_max, iu_max, ..])
                        .map(|v| v * right_coeff * bottom_coeff);
                    dst_pixel.assign(&(left_top + left_bottom + right_top + right_bottom));
                } else {
                    dst_pixel.assign(&array![0.0f32, 0., 0., 0.]);
                }

                x += i_r[(0, 0)];
                y += i_r[(1, 0)];
                w += i_r[(2, 0)];
            }
        });
}

#[inline]
pub fn even_powers_4(v: f64) -> Vector4<f64> {
    let v2 = v * v;
    let v4 = v2 * v2;
    let v6 = v4 * v2;
    let v8 = v6 * v2;
    Vector4::new(v2, v4, v6, v8)
}

pub fn estimate_new_camera_matrix_for_undistort_rectify(
    camera: &Matrix3<f64>,
    distortion_coeffs: &Vector4<f64>,
    image_size: [f64; 2],
    fov_scale: f64,
) -> Matrix3<f64> {
    let width = image_size[0];
    let height = image_size[1];

    let mut points = [
        Vector2::new(width / 2.0, 0.0),
        Vector2::new(width, height / 2.0),
        Vector2::new(width / 2.0, height),
        Vector2::new(0.0, height / 2.0),
    ];

    undistort_points(&mut points, camera, distortion_coeffs);

    let mut cn = points.iter().sum::<Vector2<f64>>() / 4.0;
    let aspect_ratio = camera[(0, 0)] / camera[(1, 1)];
    cn[0] *= aspect_ratio;

    for point in &mut points {
        point[1] *= aspect_ratio;
    }

    let f = {
        let (min_x, max_x) = points
            .iter()
            .map(|p| p[0])
            .minmax_by_key(|v| FloatOrd(*v))
            .into_option()
            .unwrap();
        let (min_y, max_y) = points
            .iter()
            .map(|p| p[1])
            .minmax_by_key(|v| FloatOrd(*v))
            .into_option()
            .unwrap();

        let f1 = width * 0.5 / (cn[0] - min_x);
        let f2 = width * 0.5 / (max_x - cn[0]);
        let f3 = height * 0.5 * aspect_ratio / (cn[1] - min_y);
        let f4 = height * 0.5 * aspect_ratio / (max_y - cn[1]);

        let fmax = [f1, f2, f3, f4]
            .iter()
            .copied()
            .max_by_key(|v| FloatOrd(*v))
            .unwrap();

        fmax / fov_scale
    };

    let mut new_c = -cn * f + Vector2::new(width, height * aspect_ratio) * 0.5;
    new_c[1] /= aspect_ratio;

    Matrix3::new(
        f,
        0.0,
        new_c[0],
        0.0,
        f / aspect_ratio,
        new_c[1],
        0.0,
        0.0,
        1.0,
    )
}

pub fn undistort_points(
    points: &mut [Vector2<f64>],
    camera: &Matrix3<f64>,
    distortion_coeffs: &Vector4<f64>,
) {
    let f = Vector2::<f64>::new(camera[(0, 0)], camera[(1, 1)]);
    let c = Vector2::<f64>::new(camera[(0, 2)], camera[(1, 2)]);

    for point in points {
        let pw = (*point - &c).component_div(&f);

        let theta_d = pw.magnitude().clamp(-FRAC_PI_2, FRAC_PI_2);

        let mut converged = false;
        let mut theta = theta_d;

        let mut scale = 0.0;

        const EPSILON: f64 = 1e-8;
        if theta_d.abs() > EPSILON {
            for _ in 0..10 {
                let theta_powers = even_powers_4(theta);
                let k_theta = distortion_coeffs.component_mul(&theta_powers);
                let theta_fix = (theta * (1.0 + &k_theta.sum()) - theta_d)
                    / (1.0 + Vector4::new(3., 5., 7., 9.).dot(&k_theta));

                theta -= theta_fix;
                if theta_fix.abs() < EPSILON {
                    converged = true;
                    break;
                }
            }

            scale = theta.tan() / theta_d;
        } else {
            converged = true;
        }

        let theta_flipped = (theta_d < 0.0 && theta > 0.0) || (theta_d > 0.0 && theta < 0.0);
        if converged && !theta_flipped {
            let pu = pw * scale;
            *point = pu;
        } else {
            *point = Vector2::new(f64::NAN, f64::NAN);
        }
    }
}
