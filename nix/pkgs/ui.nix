{
  lib,
  stdenv,
  nodejs_24,
  pnpm_10,
}:
let
  pnpm = pnpm_10;
in
stdenv.mkDerivation (finalAttrs: {
  pname = "promptkit-ui";
  version = "0.1.0";

  src = lib.cleanSource ../../ui;

  pnpmDeps = pnpm.fetchDeps {
    inherit (finalAttrs) pname version src;
    fetcherVersion = 2;
    hash = "sha256-FECxZrKcrpjM0wZDqChgdgDBpF4F8f1A93elXhkvmMw=";
  };

  nativeBuildInputs = [
    nodejs_24
    pnpm.configHook
  ];

  buildPhase = ''
    runHook preBuild

    pnpm run build

    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall

    mkdir -p $out
    cp -r dist/* $out/

    runHook postInstall
  '';
})
