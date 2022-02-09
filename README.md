<span class="badge-patreon"><a href="https://www.patreon.com/smartislav" title="Donate to this project using Patreon"><img src="https://img.shields.io/badge/patreon-donate-yellow.svg" alt="Patreon donate button" /></a></span>
![example workflow](https://github.com/gyroflow/gyroflow-ofx/actions/workflows/build.yml/badge.svg)

# Gyroflow OpenFX plugin

* Works with stabilization data exported with [gyroflow](http://gyroflow.xyz/)
* Allows you to apply the stabilization right in your OpenFX-capable video editor

# Installation

Grab the archive for your OS from the [releases page](https://github.com/gyroflow/gyroflow-ofx/releases).

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

Please see an earlier version of this [README.md](https://github.com/gyroflow/gyroflow-ofx/blob/82d31797ae586daac16e0a8b3b492a16af606f6c/README.md#with-gyroflow-0x)

# License

This software is licensed under GNU General Public License version 3 ([LICENSE](LICENSE))

# Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the GNU General Public License version 3, shall be
licensed as above, without any additional terms or conditions.
