{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs =
    {
      self,
      flake-utils,
      nixpkgs,
      crane,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain (
          p:
          p.rust-bin.stable.latest.default.override {
            extensions = [ "rust-src" ];
            targets = [ "wasm32-wasip1" ];
          }
        );
      in
      {
        devShells.default =
          (craneLib.devShell.override {
            mkShell = pkgs.mkShell.override {
              stdenv = pkgs.stdenvAdapters.useMoldLinker pkgs.clangStdenv;
            };
          })
            {
              buildInputs =
                with pkgs;
                [
                  just

                  # js
                  nodejs
                  pnpm

                  # python
                  (python313.withPackages (
                    p: with p; [
                      cython
                      setuptools
                      uv
                      typing-extensions
                      pip
                      wheel
                    ]
                  ))
                  maturin

                  # rust / c++
                  binaryen
                  cmake
                  ninja
                  buf
                  protobuf_28
                  pkg-config
                ]
                ++ lib.optional stdenv.isDarwin [
                  darwin.apple_sdk.frameworks.Security
                  darwin.apple_sdk.frameworks.CoreFoundation
                  darwin.apple_sdk.frameworks.SystemConfiguration
                  libiconv
                ];
            };
      }
    );

}
