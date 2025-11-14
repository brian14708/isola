{
  stdenv,
  fetchurl,
  autoPatchelfHook,
  lib,
  binaryen,
}:

stdenv.mkDerivation (finalAttrs: rec {
  pname = "wasi-sdk";
  version = "28";

  src = fetchurl (
    let
      platform =
        if stdenv.hostPlatform.system == "x86_64-linux" then
          {
            os = "x86_64-linux";
            hash = "sha256-xF3GYb9q/v5z3SrkeuNyyfkNtdQRH+vITJzziZGjI10=";
          }
        else if stdenv.hostPlatform.system == "aarch64-linux" then
          {
            os = "arm64-linux";
            hash = "sha256-cu9vYFMIk2FRdy7nD6C/jcp3QrVUq7zW7WO4Aq4hsog=";
          }
        else if stdenv.hostPlatform.system == "aarch64-darwin" then
          {
            os = "arm64-macos";
            hash = "sha256-CS0YWa1jON5AwW7gd5EC/xnAZ92GOtgWulr9i9jLcJ8=";
          }
        else if stdenv.hostPlatform.system == "x86_64-darwin" then
          {
            os = "x86_64-macos";
            hash = "sha256-JedAqCGYyLSqNE91OG6O+tZH98rgljgk4B9ZLujemfc=";
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
