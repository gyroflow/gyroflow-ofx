<span class="badge-patreon"><a href="https://www.patreon.com/smartislav" title="Donate to this project using Patreon"><img src="https://img.shields.io/badge/patreon-donate-yellow.svg" alt="Patreon donate button" /></a></span>
![example workflow](https://github.com/ilya-epifanov/gyroflow-ofx/actions/workflows/build.yml/badge.svg)

# Fisheye correction + gyro stabilization OpenFX plugin

* Works with stabilization data exported with [gyroflow](http://gyroflow.xyz/)
* Allows you apply the stabilization right in your OpenFX-capable video editor

## Installation

Grab the archive for your OS from the [releases page](https://github.com/ilya-epifanov/gyroflow-ofx/releases).

### Linux

    mkdir -p /usr/OFX/Plugins
    cd /usr/OFX/Plugins
    sudo unzip ${PATH_TO}/gyroflow-ofx-linux.zip

### MacOS

Copy the `GyroFlow.ofx.bundle` from the archive into the `/Library/OFX/Plugins` directory.
Create the directory if it doesn't exist yet.

### Windows

Copy the `GyroFlow.ofx.bundle` from the archive into the `C:\Program Files\Common Files\OFX\Plugins` folder.
Create the folder if it doesn't exist yet.

## License

Licensed under either of

 * Apache License, Version 2.0
   ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license
   ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
