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
  version = "34";

  src = fetchurl (
    let
      platform =
        if stdenv.hostPlatform.system == "x86_64-linux" then
          {
            os = "x86_64-linux";
            hash = "sha256-t2HjoHIduunAmgBZ5f2yv5F9G0qKe0MPs7Wq+wmEssQ=";
          }
        else if stdenv.hostPlatform.system == "aarch64-linux" then
          {
            os = "arm64-linux";
            hash = "sha256-9+JD3/VNYLzFdulNYWa2n0EPJQCuSpzu80MVvhDneXE=";
          }
        else if stdenv.hostPlatform.system == "aarch64-darwin" then
          {
            os = "arm64-macos";
            hash = "sha256-nFk5gQa0F/jxSRM4D98Al6jMD/SvnrPOAGWoWeiNSek=";
          }
        else if stdenv.hostPlatform.system == "x86_64-darwin" then
          {
            os = "x86_64-macos";
            hash = "sha256-h9J/qK3Gje5Zv78uIqbTTvcXw01r8divKlb8kp2c4Os=";
          }
        else
          throw "Unsupported platform: ${stdenv.hostPlatform.system}";
    in
    {
      url = "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${version}/wasi-sdk-${version}.0-${platform.os}.tar.gz";
      inherit (platform) hash;
    }
  );

  nativeBuildInputs = [ binaryen ] ++ lib.optionals stdenv.hostPlatform.isLinux [ autoPatchelfHook ];

  buildInputs = lib.optionals stdenv.hostPlatform.isLinux [
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
      wasm-opt "$so_file" -all -O4 --strip-debug --disable-compact-imports -o "$temp_file"
      mv "$temp_file" "$so_file"
    done
    runHook postFixup
  '';

  passthru = {
    target = "wasm32-wasip2";
    cmakeToolchain = "${finalAttrs.finalPackage}/share/cmake/wasi-sdk-p2.cmake";
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
