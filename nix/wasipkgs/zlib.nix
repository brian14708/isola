{
  stdenv,
  cmake,
  wasipkgs,
  pkg-config,
  zlib-ng,
}:
let
  inherit (wasipkgs) sdk;
in
stdenv.mkDerivation {
  pname = "${zlib-ng.pname}-wasi";
  inherit (zlib-ng) version src;
  dontStrip = true;

  nativeBuildInputs = [
    cmake
    pkg-config
  ];

  cmakeFlags = [
    "-DCMAKE_TOOLCHAIN_FILE=${sdk.cmakeToolchain}"
    "-DCMAKE_POSITION_INDEPENDENT_CODE=ON"
    "-DZLIB_COMPAT=ON"
    "-DZLIB_ENABLE_TESTS=OFF"
    "-DBUILD_SHARED_LIBS=OFF"
    "-DWITH_RUNTIME_CPU_DETECTION=OFF"
  ];
}
