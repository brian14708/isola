{
  stdenv,
  fetchurl,
  makeRustPlatform,
  maturin,
  pkg-config,
  rust-bin,
  wasipkgs,
  symlinkJoin,
}:
let
  inherit (wasipkgs) wasi-optimize-hook sdk python;
  rustToolchain = rust-bin.nightly.latest.minimal.override {
    extensions = [ "rust-src" ];
    targets = [ "wasm32-wasip1" ];
  };
  rustPlatform = makeRustPlatform {
    rustc = rustToolchain;
    cargo = rustToolchain;
  };
in
stdenv.mkDerivation rec {
  pname = "pydantic-core-wasi";
  version = "2.41.5";
  src = fetchurl {
    url = "https://github.com/pydantic/pydantic-core/archive/refs/tags/v${version}.tar.gz";
    hash = "sha256-hy9wD35Ccj4XzsUpHQBnfXkMFHaAMPTzfn3nLCk11zE=";
  };
  cargoDeps = symlinkJoin {
    name = "pydantic-core-wasi-deps";
    paths = [
      (rustPlatform.fetchCargoVendor {
        inherit src;
        hash = "sha256-Kvc0a34C6oGc9oS/iaPaazoVUWn5ABUgrmPa/YocV+Y=";
      })
      (rustPlatform.fetchCargoVendor {
        name = "rust-std";
        src = "${rustToolchain.passthru.availableComponents.rust-src}/lib/rustlib/src/rust/library";
        hash = "sha256-mFzDKitNv66qzw4CVJsqVcrm/SpHtxF4w288deY+EiM=";
      })
    ];
  };
  dontStrip = true;

  nativeBuildInputs = [
    python.host
    maturin
    rustToolchain
    rustPlatform.cargoSetupHook
    sdk
    pkg-config
    wasi-optimize-hook
  ];

  buildInputs = [ python ];

  patches = [ ./pydantic-core.patch ];

  configurePhase = ''
    runHook preConfigure

    export PYTHONPATH=${python}/lib/python3.14
    export _PYTHON_SYSCONFIGDATA_NAME=_sysconfigdata__wasi_wasm32-wasi
    export _PYTHON_HOST_PLATFORM=wasi-wasm32
    export PYO3_CROSS_LIB_DIR=${python}/lib

    export CARGO_BUILD_TARGET=wasm32-wasip1
    export CARGO_TARGET_WASM32_WASIP1_LINKER=${sdk}/bin/wasm-ld
    export CC="${sdk}/bin/clang --sysroot=${sdk}/share/wasi-sysroot"
    export AR="${sdk}/bin/llvm-ar"
    export RANLIB="${sdk}/bin/llvm-ranlib"
    export LDSHARED="${sdk}/bin/clang --sysroot=${sdk}/share/wasi-sysroot"

    export RUSTFLAGS="-Clink-self-contained=no -Crelocation-model=pic -Clink-args=-L${python}/lib -Clink-args=-L${sdk}/share/wasi-sysroot/lib/wasm32-wasip1"

    runHook postConfigure
  '';

  buildPhase = ''
    runHook preBuild

    maturin build \
      -Z build-std=std,panic_abort \
      --release \
      --target wasm32-wasip1 \
      -i python3.14 \
      --out dist

    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall

    mkdir -p $out/lib/python3.14/site-packages
    python3 -m zipfile -e \
      dist/pydantic_core-${version}-cp314-cp314-*.whl \
      $out/lib/python3.14/site-packages

    runHook postInstall
  '';
}
