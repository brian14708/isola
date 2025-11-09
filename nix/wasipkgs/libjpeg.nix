{
  stdenv,
  cmake,
  wasipkgs,
  pkg-config,
  libjpeg,
}:
let
  sdk = wasipkgs.sdk;
in
stdenv.mkDerivation {
  pname = "${libjpeg.pname}-wasi";
  version = libjpeg.version;
  src = libjpeg.src;
  dontStrip = true;

  nativeBuildInputs = [
    cmake
    pkg-config
  ];

  cmakeFlags = [
    "-DCMAKE_TOOLCHAIN_FILE=${sdk.cmakeToolchain}"
    "-DWITH_TURBOJPEG=OFF"
    "-DENABLE_SHARED=OFF"
    "-DCMAKE_POSITION_INDEPENDENT_CODE=ON"
  ];

  preConfigure = ''
    export CFLAGS="$CFLAGS -mllvm -wasm-enable-sjlj"
    export LDFLAGS="$LDFLAGS -lsetjmp"
  '';
}
