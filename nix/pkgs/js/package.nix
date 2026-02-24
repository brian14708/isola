{
  lib,
  pkgs,
  crane,
  writeShellScriptBin,
  wasipkgs,
  nukeReferences,
  python,
}:
let
  rustToolchainFor = (p: p.rust-bin.fromRustupToolchainFile ../../../rust-toolchain.toml);
  rustToolchain = rustToolchainFor pkgs;
  craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchainFor;
  src = lib.fileset.toSource {
    root = ../../..;
    fileset = lib.fileset.unions [
      ../../../Cargo.lock
      ../../../Cargo.toml
      ../../../crates/isola/wit
      (craneLib.fileset.commonCargoSources ../../../crates/xtask)
      (craneLib.fileset.commonCargoSources ../../../crates/js-runtime)
      (lib.fileset.fileFilter (file: file.hasExt "js") ../../../crates/js-runtime)
    ];
  };
in
craneLib.buildPackage {
  pname = "isola-js";
  inherit src;
  preConfigure = ''
        # The filtered source includes crates/isola/wit for component metadata, but
        # not the crate manifest/source. Create a minimal stub crate so workspace
        # member discovery succeeds when running xtask.
        if [ ! -f crates/isola/Cargo.toml ]; then
          mkdir -p crates/isola/src
          cat > crates/isola/Cargo.toml <<'EOF'
    [package]
    name = "isola"
    version.workspace = true
    edition.workspace = true
    publish = false

    [lib]
    path = "src/lib.rs"

    [lints]
    workspace = true
    EOF
          cat > crates/isola/src/lib.rs <<'EOF'
    // Stub workspace member used by the JS Nix build source filter.
    EOF
        fi
  '';
  nativeBuildInputs = [
    (writeShellScriptBin "cargo-b" "exec cargo build \"$@\"")
    nukeReferences
  ];

  env = {
    WASI_PYTHON_DEV = python.bundle.dev;
    WASI_SDK = wasipkgs.sdk;
  };

  cargoExtraArgs = "-p xtask";
  cargoBuildCommand = "cargo run -p xtask build-js";
  doCheck = false;
  cargoVendorDir = craneLib.vendorMultipleCargoDeps {
    inherit (craneLib.findCargoFiles src) cargoConfigs;
    cargoLockList = [
      ../../../Cargo.lock
      "${rustToolchain.passthru.availableComponents.rust-src}/lib/rustlib/src/rust/library/Cargo.lock"
    ];
  };

  installPhase = ''
    runHook preInstall

    install -Dm644 target/js.wasm $out/bin/js.wasm
    find $out/ -type f -print -exec nuke-refs '{}' +

    runHook postInstall
  '';
}
