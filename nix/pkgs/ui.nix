{
  lib,
  stdenv,
  nodejs_24,
  pnpm_10,
  fetchPnpmDeps,
  pnpmConfigHook,
}:
let
  pnpm = pnpm_10;
in
stdenv.mkDerivation (finalAttrs: {
  pname = "promptkit-ui";
  version = "0.1.0";

  src = lib.cleanSource ../../ui;

  pnpmDeps = fetchPnpmDeps {
    inherit (finalAttrs) pname version src;
    inherit pnpm;
    fetcherVersion = 2;
    hash = "sha256-woRYGQ8u+kq2Rc6Wg9JzAwJmyaRsNGgGSoEPJAfXmSk=";
  };

  nativeBuildInputs = [
    nodejs_24
    pnpm
    pnpmConfigHook
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
