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
(craneLib.devShell.override {
  mkShell = pkgs.mkShell.override {
    stdenv =
      if pkgs.stdenv.isDarwin then pkgs.stdenv else pkgs.stdenvAdapters.useMoldLinker pkgs.stdenv;
  };
})
  {
    buildInputs = with pkgs; [
      just
      mdbook
      (python.withPackages (p: with p; [ uv ]))
    ];

    env = {
      UV_PYTHON = python.interpreter;
      WASI_PYTHON_DEV = "${bundle.dev}";
      WASI_PYTHON_RUNTIME = "${bundle}";
    };
  }
