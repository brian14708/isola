{
  crane,
  pkgs,
  packages,
}:
let
  inherit (packages.python) bundle;
  craneLib = (crane.mkLib pkgs).overrideToolchain (
    p: p.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml
  );
  python = pkgs.wasipkgs.python.host;
in
craneLib.devShell {
  buildInputs = with pkgs; [
    just
    mdbook
    (python.withPackages (p: with p; [ uv ]))
    maturin
  ];

  env = {
    WASI_SDK = pkgs.wasipkgs.sdk;
    WASI_PYTHON_DEV = "${bundle.dev}";
    WASI_PYTHON_RUNTIME = "${bundle}";
  };
}
