<span class="badge-patreon"><a href="https://www.patreon.com/smartislav" title="Donate to this project using Patreon"><img src="https://img.shields.io/badge/patreon-donate-yellow.svg" alt="Patreon donate button" /></a></span>
![example workflow](https://github.com/ilya-epifanov/gyroflow-ofx/actions/workflows/build.yml/badge.svg)

# Fisheye correction + gyro stabilization OpenFX plugin

* Works with stabilization data exported with [gyroflow](http://gyroflow.xyz/)
* Allows you apply the stabilization right in your OpenFX-capable video editor

# Installation

Grab the archive for your OS from the [releases page](https://github.com/ilya-epifanov/gyroflow-ofx/releases).

## Linux

    mkdir -p /usr/OFX/Plugins
    cd /usr/OFX/Plugins
    sudo unzip ${PATH_TO}/gyroflow-ofx-linux.zip

## MacOS

Copy the `GyroFlow.ofx.bundle` from the archive into the `/Library/OFX/Plugins` directory.
Create the directory if it doesn't exist yet.

## Windows

Copy the `GyroFlow.ofx.bundle` from the archive into the `C:\Program Files\Common Files\OFX\Plugins` folder.
Create the folder if it doesn't exist yet.

# Usage

## With Gyroflow 1.0

### Export `.gyroflow` file in the Gyroflow app

Click the `Export .gyroflow file (including gyro data)` in the Gyroflow app.

### Basic plugin usage

First you need to apply the plugin to the clip.
In DaVinci Resolve you can do that by going to the Fusion tab and inserting the "Warp -> Gyroflow (1.0)" (or "Warp -> Fisheye stabilizer (1.0)") after the media input node.

### Load the .gyroflow file

In DaVinci Resolve Fusion, go to the `Gyroflow` (or `Fisheye stabilizer`) node settings. Select the `.gyroflow` file in the `Gyroflow file` entry.

## With Gyroflow 0.x

### Export keyframes in the Gyroflow app

Instead of exporting video, click the `Export (hopefully) stabilized keyframes for the whole clip` in the Gyroflow app.

### Basic plugin usage

First you need to apply the plugin to the clip.
In DaVinci Resolve you can do that by going to the Fusion tab and inserting the "Warp -> Fisheye stabilizer (0.x)" after media input node.
You'll see some distortion correction being applied, probably not the correct one for your setup.

### Per-camera setup

Fill in the camera matrix, calibration dimensions and distortion coefficients. Steal the values from the [camera profile .json](https://github.com/ElvinC/gyroflow/tree/master/camera_presets).
You'll need to divide the top row (`K[0][*]`) of camera matrix and calibration width by `input_horizontal_stretch` if it's not equal to `1` in the camera json.
If it's missing or equal to `1`, you don't need to divide anything.

Example:

```jsonc
    "calib_dimension": {
        "w": 2880, // 2880 / 0.75 = 3840
        "h": 2160
    },
    // ...
    "input_horizontal_stretch": 0.75, // !!!
    // ...
    "fisheye_params": {
        // ...
        "camera_matrix": [
            [
                1503.6509757882493, // 1503.6509757882493 / 0.75 = 2004.867967718
                0.0,
                1440.0 // 1440.0 / 0.75 = 1920.0
            ],
            [
                0.0,
                1504.4333376346274,
                1080.0
            ],
            [
                0.0,
                0.0,
                1.0
            ]
        ],
        "distortion_coeffs": [
            -0.0222830144220073,
            -0.022383716628994344,
            -0.015155481249009247,
            0.02079247454299656
        ]
    },
    // ...
```

translates into the following plugin parameters:

```
Camera matrix:
K[0][0] = 2004.867967718
K[0][1] = 0.0
K[0][2] = 1920.0
K[1][0] = 0.0
K[1][1] = 1504.4333376346274
K[1][2] = 1080.0
K[2][0] = 0.0
K[2][1] = 0.0
K[2][2] = 1.0

Camera calibration dimensions:
Calibration width = 3840.0
Calibration height = 2160.0

Distortion coefficients:
Distortion 0 = -0.0222830144220073
Distortion 1 = -0.022383716628994344
Distortion 2 = -0.015155481249009247
Distortion 3 = 0.02079247454299656
```

Save this as a preset. In Fusion, right-click the node and `Settings -> Save As...`.

You should see the distortion being corrected properly now (straight lines should still be straight no matter how close to the frame edge).

### Load the gyro data

In DaVinci Resolve Fusion, go to the `Fisheye stabilizer` node settings again. Click the diamond button next to `Correction W/X/Y/Z` parameters in the `Correction quaternion` section. 
Open the `Spline` pane.
Right click `Fisheyestabilizer*/Correction W` -> `Import Spline...`, load the corresponding `.w.spl` file. Do the same for `Correction X/Y/Z` parameters. Take care to load the correct `.(x|y|z).spl` files.
Voilà.
Now adjust the `FOV scale` parameter to your liking.

# License

Version 0.1 of the plugin is licensed under either of

 * Apache License, Version 2.0
   ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license
   ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

Version 1.0 of the plugin is licensed under either of

 * GNU General Public License version 3
   ([LICENSE-GPL](LICENSE-GPL))

# Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
