{
  stdenv,
  fetchurl,
  autoPatchelfHook,
  lib,
  binaryen,
  ncurses,
}:

stdenv.mkDerivation (finalAttrs: rec {
  pname = "wasi-sdk";
  version = "32";

  src = fetchurl (
    let
      platform =
        if stdenv.hostPlatform.system == "x86_64-linux" then
          {
            os = "x86_64-linux";
            hash = "sha256-VfxSPr+8mPadEDT8/Lg9H/VhDNmrfs7vbNCXowuk75M=";
          }
        else if stdenv.hostPlatform.system == "aarch64-linux" then
          {
            os = "arm64-linux";
            hash = "sha256-sgcIZebLDB6Xo45qyNnDep380HUnZOurxrrNPmDO25Y=";
          }
        else if stdenv.hostPlatform.system == "aarch64-darwin" then
          {
            os = "arm64-macos";
            hash = "sha256-ODvn/gCuBGkeGFkWT9mZcKUoxV1iS4hu1Z95M4mLkz0=";
          }
        else if stdenv.hostPlatform.system == "x86_64-darwin" then
          {
            os = "x86_64-macos";
            hash = "sha256-o2yasQakCr6KBRJ5wjPGurcZpXiOzAeI6NFBeW61Wxs=";
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

  buildInputs = lib.optionals stdenv.isLinux [
    stdenv.cc.cc
    ncurses
  ];

  dontBuild = true;
  dontConfigure = true;
  dontStrip = true;
  dontUpdateAutotoolsGnuConfigScripts = true;

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
    target = "wasm32-wasip1";
    cmakeToolchain = "${finalAttrs.finalPackage}/share/cmake/wasi-sdk-p1.cmake";
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
