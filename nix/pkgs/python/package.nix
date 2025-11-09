{
  lib,
  craneLib,
  writeShellScriptBin,
  python314,
  nukeReferences,
  callPackage,
}:
let
  bundle = callPackage ./bundle.nix { };
  src = lib.fileset.toSource {
    root = ../../..;
    fileset = lib.fileset.unions [
      (craneLib.fileset.commonCargoSources ../../..)
      ../../../specs
    ];
  };
in
craneLib.buildPackage {
  pname = "promptkit-python";
  version = "0.1.0";

  inherit src;
  cargoVendorDir = craneLib.vendorCargoDeps { inherit src; };

  nativeBuildInputs = [
    (writeShellScriptBin "cargo-b" "exec cargo build \"$@\"")
    python314
    nukeReferences
  ];

  WASI_PYTHON_DEV = bundle.dev;
  WASI_PYTHON_RUNTIME = bundle;

  cargoBuildCommand = "cargo xtask build-python";

  installPhase = ''
    runHook preInstall

    install -Dm644 target/promptkit_python.wasm $out/lib/promptkit_python.wasm
    find $out/ -type f -print -exec nuke-refs '{}' +

    runHook postInstall
  '';

  doCheck = false;

  passthru = {
    inherit bundle;
  };
}
