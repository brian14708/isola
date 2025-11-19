{
  lib,
  pkgs,
  crane,
  writeShellScriptBin,
  wasipkgs,
  nukeReferences,
  callPackage,
}:
let
  bundle = callPackage ./bundle.nix { };

  rustToolchainFor = (
    p:
    p.rust-bin.nightly.latest.minimal.override {
      extensions = [ "rust-src" ];
      targets = [ "wasm32-wasip1" ];
    }
  );
  rustToolchain = rustToolchainFor pkgs;
  craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchainFor;
  src = lib.fileset.toSource {
    root = ../../..;
    fileset = lib.fileset.unions [
      ../../../specs
      ../../../Cargo.lock
      ../../../Cargo.toml
      (craneLib.fileset.commonCargoSources ../../../crates/xtask)
      (craneLib.fileset.commonCargoSources ../../../crates/python)
    ];
  };
in
craneLib.buildPackage {
  pname = "promptkit-python";
  inherit src;
  nativeBuildInputs = [
    (writeShellScriptBin "cargo-b" "exec cargo build \"$@\"")
    wasipkgs.python.host
    nukeReferences
  ];

  env = {
    PYO3_PYTHON = "${wasipkgs.python.host}/bin/python3";
    WASI_PYTHON_DEV = bundle.dev;
    WASI_PYTHON_RUNTIME = bundle;
  };

  cargoExtraArgs = "-p xtask";
  cargoBuildCommand = "cargo run -p xtask build-python";
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

    install -Dm644 target/promptkit_python.wasm $out/lib/promptkit_python.wasm
    find $out/ -type f -print -exec nuke-refs '{}' +

    runHook postInstall
  '';

  passthru = {
    inherit bundle;
  };
}
