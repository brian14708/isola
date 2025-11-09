{
  lib,
  craneLib,
  protobuf,
}:
let
  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      (craneLib.fileset.commonCargoSources ../..)
      ../../specs
    ];
  };
in
craneLib.buildPackage {
  pname = "promptkit-server";
  version = "0.1.0";

  inherit src;
  cargoVendorDir = craneLib.vendorCargoDeps { inherit src; };

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
