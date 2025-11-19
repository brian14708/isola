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
    "-DWITH_TURBOJPEG=OFF"
    "-DENABLE_SHARED=OFF"
    "-DWITH_TOOLS=OFF"
    "-DCMAKE_POSITION_INDEPENDENT_CODE=ON"
  ];
}
