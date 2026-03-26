{
  crane,
  stdenv,
  pkgs,
  makeRustPlatform,
  maturin,
  pkg-config,
  runCommand,
  rust-bin,
  wasipkgs,
}:
let
  inherit (wasipkgs) wasi-optimize-hook sdk python;
  inherit (python) host;
  pydanticCore = host.pkgs.pydantic-core;
  inherit (pydanticCore) version src;
  rustToolchain = rust-bin.fromRustupToolchainFile ../../../rust-toolchain.toml;
  craneLib = (crane.mkLib pkgs).overrideToolchain (
    p: p.rust-bin.fromRustupToolchainFile ../../../rust-toolchain.toml
  );
  rustPlatform = makeRustPlatform {
    rustc = rustToolchain;
    cargo = rustToolchain;
  };
  packageCargoDeps = rustPlatform.fetchCargoVendor {
    inherit src;
    hash = "sha256-Kvc0a34C6oGc9oS/iaPaazoVUWn5ABUgrmPa/YocV+Y=";
  };
  mergedCargoDeps = craneLib.vendorMultipleCargoDeps {
    inherit (craneLib.findCargoFiles src) cargoConfigs;
    cargoLockList = [
      "${src}/Cargo.lock"
      "${rustToolchain.passthru.availableComponents.rust-src}/lib/rustlib/src/rust/library/Cargo.lock"
    ];
  };
in
stdenv.mkDerivation rec {
  pname = "${pydanticCore.pname}-wasi";
  inherit version src;
  cargoDeps = runCommand "pydantic-core-wasi-deps" { } ''
    mkdir -p "$out/.cargo"
    ln -s ${mergedCargoDeps}/config.toml "$out/.cargo/config.toml"
    ln -s ${packageCargoDeps}/Cargo.lock "$out/Cargo.lock"

    for dep in ${mergedCargoDeps}/*; do
      name="$(basename "$dep")"
      case "$name" in
        config.toml) continue ;;
      esac
      ln -s "$dep" "$out/$name"
    done
  '';
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
