{
  stdenv,
  fetchFromGitHub,
  pkg-config,
  wasipkgs,
}:
let
  inherit (wasipkgs)
    wasi-optimize-hook
    sdk
    zlib
    libjpeg
    freetype
    python
    ;
  host = python.host;
in
stdenv.mkDerivation rec {
  pname = "pillow-wasi";
  version = "12.0.0";
  src = fetchFromGitHub {
    owner = "python-pillow";
    repo = "pillow";
    tag = version;
    hash = "sha256-58mjwHErEZPkkGBVZznkkMQN5Zo4ZBBiXnhqVp1F81g=";
  };
  dontStrip = true;

  nativeBuildInputs = [
    wasi-optimize-hook
    (host.withPackages (ps: with ps; [ setuptools ]))
    sdk
    pkg-config
  ];

  buildInputs = [
    python
    zlib
    libjpeg
    freetype
  ];

  patches = [ ./pillow.patch ];

  configurePhase = ''
    runHook preConfigure

    export PYTHONPATH=${python}/lib/python3.14
    export _PYTHON_SYSCONFIGDATA_NAME=_sysconfigdata__wasi_wasm32-wasi
    export _PYTHON_HOST_PLATFORM=wasi-wasm32

    export CC="${sdk}/bin/clang --sysroot=${sdk}/share/wasi-sysroot --target=wasm32-wasip1"
    export AR="${sdk}/bin/llvm-ar"
    export RANLIB="${sdk}/bin/llvm-ranlib"
    export LD="${sdk}/bin/wasm-ld"

    export CFLAGS="-fPIC -I${python}/include/python3.14"
    export LDFLAGS="-L${python}/lib -lpython3.14 -ldl -lz"

    runHook postConfigure
  '';

  installPhase = ''
    runHook preInstall

    python3 setup.py install \
      --prefix=$out \
      --single-version-externally-managed \
      --root=/

    find $out/ -type d -name "__pycache__" -exec rm -rf {} +
    runHook postInstall
  '';
}
