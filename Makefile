target/bundle-common/Info.plist: res/Info.plist Cargo.toml Cargo.lock
	mkdir -p target/bundle-common
	sed "s/####VERSION####/`cargo read-manifest | jq -r '.version'`/" res/Info.plist > $@

target/gyroflow-ofx-linux.zip: target/release/libgyroflow_ofx.so LICENSE LICENSE-APACHE LICENSE-MIT README.md target/bundle-common/Info.plist Makefile
	rm -Rf target/gyroflow-ofx-linux
	rm -f target/gyroflow-ofx-linux.zip
	mkdir -p target/gyroflow-ofx-linux/GyroFlow.ofx.bundle/Contents/Linux-x86-64
	cp target/release/libgyroflow_ofx.so target/gyroflow-ofx-linux/GyroFlow.ofx.bundle/Contents/Linux-x86-64/
	cp target/bundle-common/Info.plist LICENSE LICENSE-MIT LICENSE-APACHE README.md target/gyroflow-ofx-linux/GyroFlow.ofx.bundle/Contents/
	cd target/gyroflow-ofx-linux && zip -r ../gyroflow-ofx-linux.zip .

target/gyroflow-ofx-macosx.zip: target/release/libgyroflow_ofx.dylib LICENSE LICENSE-APACHE LICENSE-MIT README.md target/bundle-common/Info.plist Makefile
	rm -Rf target/gyroflow-ofx-macosx
	rm -f target/gyroflow-ofx-macosx.zip
	mkdir -p target/gyroflow-ofx-macosx/GyroFlow.ofx.bundle/Contents/MacOS-x86-64
	cp target/release/libgyroflow_ofx.dylib target/gyroflow-ofx-macosx/GyroFlow.ofx.bundle/Contents/MacOS-x86-64/
	cp target/bundle-common/Info.plist LICENSE LICENSE-MIT LICENSE-APACHE README.md target/gyroflow-ofx-macosx/GyroFlow.ofx.bundle/Contents/
	cd target/gyroflow-ofx-macosx && zip -r ../gyroflow-ofx-macosx.zip .

target/gyroflow-ofx-windows.zip: target/release/gyroflow_ofx.dll LICENSE LICENSE-APACHE LICENSE-MIT README.md target/bundle-common/Info.plist Makefile
	rm -Rf target/gyroflow-ofx-windows
	rm -f target/gyroflow-ofx-windows.zip
	mkdir -p target/gyroflow-ofx-windows/GyroFlow.ofx.bundle/Contents/Win64
	cp target/release/gyroflow_ofx.dll target/gyroflow-ofx-windows/GyroFlow.ofx.bundle/Contents/Win64/
	cp target/bundle-common/Info.plist LICENSE LICENSE-MIT LICENSE-APACHE README.md target/gyroflow-ofx-windows/GyroFlow.ofx.bundle/Contents/
	cd target/gyroflow-ofx-windows && zip -r ../gyroflow-ofx-windows.zip .
