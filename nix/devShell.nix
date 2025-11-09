{
  craneLib,
  pkgs,
  packages,
}:
let
  bundle = packages.python.bundle;
in
craneLib.devShell {
  buildInputs = with pkgs; [
    just
    pnpm
    nodejs_24
    (python314.withPackages (p: with p; [ uv ]))
    buf
    protobuf
    cmake
    ninja
  ];

  env = {
    UV_PYTHON = pkgs.python314.interpreter;
    WASI_PYTHON_DEV = "${bundle.dev}";
    WASI_PYTHON_RUNTIME = "${bundle}";
  };
}
