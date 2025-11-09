{
  makeSetupHook,
  binaryen,
  writeText,
}:
makeSetupHook
  {
    name = "wasi-optimize-hook";
    propagatedBuildInputs = [ binaryen ];
  }
  (
    writeText "wasi-optimize-hook.sh" ''
      wasiOptimizePhase() {
        echo "Optimizing WASI .so files with wasm-opt..."

        find "$prefix" -type f -name "*.so" -print0 | while IFS= read -r -d "" so_file; do
          echo "Optimizing: $so_file"
          temp_file="''${so_file}.tmp"
          wasm-opt "$so_file" -all -O4 --strip-debug -o "$temp_file"
          mv "$temp_file" "$so_file"
        done
      }

      # Run during fixup phase, after installation
      preFixupHooks+=(wasiOptimizePhase)
    ''
  )
