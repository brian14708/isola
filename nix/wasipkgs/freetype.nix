{
  stdenv,
  pkg-config,
  wasipkgs,
  freetype,
}:
let
  inherit (wasipkgs) sdk zlib;
in
stdenv.mkDerivation {
  pname = "${freetype.pname}-wasi";
  version = freetype.version;
  src = freetype.src;
  enableParallelBuilding = true;
  dontStrip = true;

  propagatedBuildInputs = [ zlib ];

  patches = [ ./freetype.patch ];

  nativeBuildInputs = [
    pkg-config
  ];

  configureFlags = [
    "--host=${sdk.target}"
    "--with-brotli=no"
    "--with-bzip2=no"
    "--with-png=no"
    "--with-harfbuzz=no"
  ];

  preConfigure = ''
    export CC_BUILD=${stdenv.cc}/bin/cc
    export CC=${sdk}/bin/clang
    export LD=${sdk}/bin/ld
    export AR=${sdk}/bin/ar
    export RANLIB=${sdk}/bin/ranlib
    export CFLAGS="$CFLAGS -fPIC -mllvm -wasm-enable-sjlj"
    export LDFLAGS="$LDFLAGS -lsetjmp"
  '';
}
