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
  rustToolchainFor = (p: p.rust-bin.fromRustupToolchainFile ../../../rust-toolchain.toml);
  rustToolchain = rustToolchainFor pkgs;
  craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchainFor;
  baseSrc = lib.fileset.toSource {
    root = ../../..;
    fileset = lib.fileset.unions [
      ../../../wit
      ../../../Cargo.lock
      ../../../Cargo.toml
      (craneLib.fileset.commonCargoSources ../../../crates/xtask)
      (craneLib.fileset.commonCargoSources ../../../crates/python)
    ];
  };
  src = pkgs.runCommand "isola-python-src" { } ''
        mkdir -p "$out"
        cp -r --no-preserve=mode ${baseSrc}/. "$out"

        make_dummy_crate() {
          local crate_dir="$1"
          local crate_name="$2"

          mkdir -p "$out/$crate_dir/src"
          cat > "$out/$crate_dir/Cargo.toml" <<EOF
    [package]
    name = "$crate_name"
    version = "0.0.0"
    edition = "2024"

    [lib]
    path = "src/lib.rs"
    EOF
          cat > "$out/$crate_dir/src/lib.rs" <<EOF
    pub fn stub() {}
    EOF
        }

        make_dummy_crate "crates/c-api" "isola-c-api"
        make_dummy_crate "crates/c-api-export" "isola-c-api-export"
        make_dummy_crate "crates/isola" "isola"
        make_dummy_crate "crates/server" "isola-server"
  '';
in
craneLib.buildPackage {
  pname = "isola-python";
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

    install -Dm644 target/isola_python.wasm $out/lib/isola_python.wasm
    find $out/ -type f -print -exec nuke-refs '{}' +

    runHook postInstall
  '';

  passthru = {
    inherit bundle;
  };
}
