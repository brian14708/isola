{
  stdenv,
  cmake,
  wasipkgs,
  pkg-config,
  libjpeg,
}:
let
  inherit (wasipkgs) sdk;
in
stdenv.mkDerivation {
  pname = "${libjpeg.pname}-wasi";
  inherit (libjpeg) version src;
  dontStrip = true;

  nativeBuildInputs = [
    cmake
    pkg-config
  ];

  cmakeFlags = [
    "-DCMAKE_TOOLCHAIN_FILE=${sdk.cmakeToolchain}"
    # libjpeg-turbo 3.1.x misdetects x86_64-wasi as a usable SIMD target and
    # then fails while compiling the simdcoverage helper; the scalar build is
    # sufficient for the WASI guest bundle.
    "-DWITH_SIMD=OFF"
    "-DWITH_TURBOJPEG=OFF"
    "-DENABLE_SHARED=OFF"
    "-DWITH_TOOLS=OFF"
    "-DCMAKE_POSITION_INDEPENDENT_CODE=ON"
  ];
}
