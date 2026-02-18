{
  lib,
  pkgs,
  crane,
  python,
}:
let
  craneLib = (crane.mkLib pkgs).overrideToolchain (
    p: p.rust-bin.fromRustupToolchainFile ../../rust-toolchain.toml
  );
  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../specs
      ../../Cargo.lock
      ../../Cargo.toml
      (craneLib.fileset.commonCargoSources ../../crates/cbor)
      (craneLib.fileset.commonCargoSources ../../crates/trace)
      (craneLib.fileset.commonCargoSources ../../crates/request)
      (craneLib.fileset.commonCargoSources ../../crates/promptkit)
      (craneLib.fileset.commonCargoSources ../../crates/c-api)
      (craneLib.fileset.commonCargoSources ../../crates/c-api-export)
    ];
  };
in
craneLib.buildPackage {
  pname = "isola-c-api";
  inherit src;
  strictDeps = true;

  CARGO_PROFILE = "release-lto";
  cargoExtraArgs = "-p isola-c-api-export";
  installPhase = ''
    runHook preInstall

    mkdir -p $out/lib -p $out/share/promptkit

    cp target/release-lto/libpromptkit.* $out/lib/
    rm $out/lib/libpromptkit.d
    runHook postInstall

    cp -r ${../../crates/c-api/include} $out/include
    cp -r ${python.bundle} $out/share/promptkit/python
    cp ${python}/lib/promptkit_python.wasm $out/share/promptkit/python.wasm
  '';
}
