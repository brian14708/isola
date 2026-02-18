{
  lib,
  pkgs,
  crane,
}:
let
  craneLib = (crane.mkLib pkgs).overrideToolchain (
    p: p.rust-bin.fromRustupToolchainFile ../../rust-toolchain.toml
  );
  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../wit
      ../../Cargo.lock
      ../../Cargo.toml
      (craneLib.fileset.commonCargoSources ../../crates/cbor)
      (craneLib.fileset.commonCargoSources ../../crates/trace)
      (craneLib.fileset.commonCargoSources ../../crates/request)
      (craneLib.fileset.commonCargoSources ../../crates/isola)
      (craneLib.fileset.commonCargoSources ../../crates/server)
    ];
  };
in
craneLib.buildPackage {
  pname = "isola-server";
  inherit src;
  strictDeps = true;

  CARGO_PROFILE = "release-lto";
  cargoExtraArgs = "-p isola-server";
  installPhase = ''
    runHook preInstall

    install -Dm755 target/release-lto/isola-server $out/bin/isola-server

    runHook postInstall
  '';
}
