{
  lib,
  pkgs,
  crane,
  protobuf,
}:
let
  craneLib = (crane.mkLib pkgs).overrideToolchain (p: p.rust-bin.nightly.latest.minimal);
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
      (craneLib.fileset.commonCargoSources ../../crates/server)
    ];
  };
in
craneLib.buildPackage {
  pname = "promptkit-server";
  inherit src;
  strictDeps = true;

  nativeBuildInputs = [
    protobuf
  ];

  CARGO_PROFILE = "release-lto";
  cargoExtraArgs = "-p promptkit-server";
  installPhase = ''
    runHook preInstall

    install -Dm755 target/release-lto/promptkit-server $out/bin/promptkit-server

    runHook postInstall
  '';
}
