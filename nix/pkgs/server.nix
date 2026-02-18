{
  lib,
  pkgs,
  crane,
  protobuf,
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
      (craneLib.fileset.commonCargoSources ../../crates/isola-trace)
      (craneLib.fileset.commonCargoSources ../../crates/isola-request)
      (craneLib.fileset.commonCargoSources ../../crates/isola)
      (craneLib.fileset.commonCargoSources ../../crates/isola-server)
    ];
  };
in
craneLib.buildPackage {
  pname = "isola-server";
  inherit src;
  strictDeps = true;

  nativeBuildInputs = [
    protobuf
  ];

  CARGO_PROFILE = "release-lto";
  cargoExtraArgs = "-p isola-server";
  installPhase = ''
    runHook preInstall

    install -Dm755 target/release-lto/isola-server $out/bin/isola-server

    runHook postInstall
  '';
}
