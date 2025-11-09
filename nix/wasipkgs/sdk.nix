{
  stdenv,
  fetchurl,
  autoPatchelfHook,
  lib,
  wasiTarget ? "p2",
  binaryen,
}:

stdenv.mkDerivation (finalAttrs: rec {
  pname = "wasi-sdk";
  version = "27";

  src = fetchurl (
    let
      platform =
        if stdenv.hostPlatform.system == "x86_64-linux" then
          {
            os = "x86_64-linux";
            hash = "sha256-t9TZRMiFA+TyHYSvB6wpPjRAsbYhC/1/544K/ZLCO8I=";
          }
        else if stdenv.hostPlatform.system == "aarch64-linux" then
          {
            os = "arm64-linux";
            hash = "sha256-TPTFU8RkDmPngEQhRvh9g/3/Vzf5iMBqbjsvAijjdmU=";
          }
        else if stdenv.hostPlatform.system == "aarch64-darwin" then
          {
            os = "arm64-macos";
            hash = "sha256-BVw9wnZncsOOcaBdNT41wyLHssZFijaiaoNvmAilUPg=";
          }
        else if stdenv.hostPlatform.system == "x86_64-darwin" then
          {
            os = "x86_64-macos";
            hash = "sha256-Fj39R/mJsaaCdEwa4fDgmoP/XEu6ydzYVGkJq1TNpaE=";
          }
        else
          throw "Unsupported platform: ${stdenv.hostPlatform.system}";
    in
    {
      url = "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${version}/wasi-sdk-${version}.0-${platform.os}.tar.gz";
      inherit (platform) hash;
    }
  );

  nativeBuildInputs = [ binaryen ] ++ lib.optionals stdenv.isLinux [ autoPatchelfHook ];

  buildInputs = lib.optionals stdenv.isLinux [ stdenv.cc.cc ];

  dontBuild = true;
  dontConfigure = true;
  dontStrip = true;

  installPhase = ''
    runHook preInstall
    mkdir -p $out
    cp -r * $out/
    runHook postInstall
  '';

  fixupPhase = ''
    runHook preFixup
    find "$out/share/wasi-sysroot/lib" -type f -name "*.so" -print0 | while IFS= read -r -d "" so_file; do
      echo "Optimizing: $so_file"
      temp_file="''${so_file}.tmp"
      wasm-opt "$so_file" -all -O4 --strip-debug -o "$temp_file"
      mv "$temp_file" "$so_file"
    done
    runHook postFixup
  '';

  passthru = {
    target = "wasm32-wasi${wasiTarget}";
    cmakeToolchain = "${finalAttrs.finalPackage}/share/cmake/wasi-sdk-${wasiTarget}.cmake";
  };

  meta = with lib; {
    description = "WASI-enabled WebAssembly C/C++ toolchain";
    homepage = "https://github.com/WebAssembly/wasi-sdk";
    license = with licenses; [
      asl20
      mit
    ];
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
      "aarch64-darwin"
      "x86_64-darwin"
    ];
  };
})
