{
  lib,
  stdenv,
  nodejs_24,
  pnpm_10,
  npmHooks,
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
    hash = "sha256-ccB9ci6GM7aJL0sScwhy52HHhzR7Tg2W/V6yxl3r9Zc=";
  };

  nativeBuildInputs = [
    nodejs_24
    pnpm.configHook
    npmHooks.npmInstallHook
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
