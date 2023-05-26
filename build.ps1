
$Env:LIBCLANG_PATH = "$PSScriptRoot\..\gyroflow\ext\llvm\bin"
$Env:Path = "$Env:LIBCLANG_PATH;$Env:Path"
$Env:CARGO_ENCODED_RUSTFLAGS = "-L" + [char]0x1f + "D:\Programy\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC\14.32.31326\lib\x64"
cargo build --release
Copy-Item -Path "E:\Temp\release\gyroflow_ofx.dll" -Destination "C:\Program Files\Common Files\OFX\Plugins\Gyroflow.ofx.bundle\Contents\Win64\Gyroflow.ofx" -Force

