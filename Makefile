target/bundle-common/Info.plist: res/Info.plist Cargo.toml Cargo.lock
	mkdir -p target/bundle-common
	sed "s/####VERSION####/`cargo read-manifest | jq -r '.version'`/" res/Info.plist > $@

target/gyroflow-ofx-linux.zip: target/release/libgyroflow_ofx.so LICENSE README.md target/bundle-common/Info.plist Makefile
	rm -Rf target/gyroflow-ofx-linux
	rm -f target/gyroflow-ofx-linux.zip
	mkdir -p target/gyroflow-ofx-linux/GyroFlow.ofx.bundle/Contents/Linux-x86-64
	cp target/release/libgyroflow_ofx.so target/gyroflow-ofx-linux/GyroFlow.ofx.bundle/Contents/Linux-x86-64/GyroFlow.ofx
	cp target/bundle-common/Info.plist LICENSE README.md target/gyroflow-ofx-linux/GyroFlow.ofx.bundle/Contents/
	cd target/gyroflow-ofx-linux && zip -r ../gyroflow-ofx-linux.zip .

target/gyroflow-ofx-macosx.zip: target/x86_64-apple-darwin/release/libgyroflow_ofx.dylib target/aarch64-apple-darwin/release/libgyroflow_ofx.dylib LICENSE README.md target/bundle-common/Info.plist Makefile
	rm -Rf target/gyroflow-ofx-macosx
	rm -f target/gyroflow-ofx-macosx.zip
	mkdir -p target/gyroflow-ofx-macosx/GyroFlow.ofx.bundle/Contents/MacOS
	lipo target/{x86_64,aarch64}-apple-darwin/release/libgyroflow_ofx.dylib -create -output target/gyroflow-ofx-macosx/GyroFlow.ofx.bundle/Contents/MacOS/GyroFlow.ofx
	cp target/bundle-common/Info.plist LICENSE README.md target/gyroflow-ofx-macosx/GyroFlow.ofx.bundle/Contents/

	codesign -vvvv --strict --options=runtime --timestamp --force -s ${SIGNING_FINGERPRINT} target/gyroflow-ofx-macosx/GyroFlow.ofx.bundle/Contents/MacOS/GyroFlow.ofx

	cd target/gyroflow-ofx-macosx && zip -r ../gyroflow-ofx-macosx.zip .

	codesign -vvvv --strict --options=runtime --timestamp --force -s ${SIGNING_FINGERPRINT} ../gyroflow-ofx-macosx.zip
	codesign -vvvv --deep --verify ../gyroflow-ofx-macosx.zip

target/gyroflow-ofx-windows.zip: target/release/gyroflow_ofx.dll LICENSE README.md target/bundle-common/Info.plist Makefile
	rm -Rf target/gyroflow-ofx-windows
	rm -f target/gyroflow-ofx-windows.zip
	mkdir -p target/gyroflow-ofx-windows/GyroFlow.ofx.bundle/Contents/Win64
	cp target/release/gyroflow_ofx.dll target/gyroflow-ofx-windows/GyroFlow.ofx.bundle/Contents/Win64/GyroFlow.ofx
	cp target/bundle-common/Info.plist LICENSE README.md target/gyroflow-ofx-windows/GyroFlow.ofx.bundle/Contents/
	cd target/gyroflow-ofx-windows && zip -r ../gyroflow-ofx-windows.zip .
