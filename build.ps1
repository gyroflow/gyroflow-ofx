
$Env:LIBCLANG_PATH = "$PSScriptRoot\..\gyroflow\ext\llvm-13-win64\bin"
$Env:Path = "$Env:LIBCLANG_PATH;$Env:Path"
cargo build --release
Copy-Item -Path "target\release\gyroflow_ofx.dll" -Destination "C:\Program Files\Common Files\OFX\Plugins\GyroFlow.ofx.bundle\Contents\Win64\GyroFlow.ofx" -Force
