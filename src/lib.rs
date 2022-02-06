extern crate ofx;

use ofx::*;

mod fisheye;
mod fisheyestab;
mod fisheyestab_v1;

register_modules!(
    fisheyestab,
    fisheyestab_v1
);
