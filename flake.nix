{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    crane.url = "github:ipetkov/crane";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.rust-analyzer-src.follows = "";
    };
  };
  outputs =
    inputs@{ flake-parts, crane, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];
      perSystem =
        {
          config,
          self',
          inputs',
          pkgs,
          system,
          ...
        }:
        let
          rust-toolchain = (
            with inputs'.fenix.packages;
            combine [
              (stable.withComponents [
                "cargo"
                "clippy"
                "rustc"
                "rustfmt"
                "rust-src"
              ])
              targets.wasm32-wasip1.stable.rust-std
            ]
          );
          craneLib = (crane.mkLib pkgs).overrideToolchain rust-toolchain;
        in
        {

          devShells.default =
            (craneLib.devShell.override {
              mkShell = pkgs.mkShell.override {
                stdenv = pkgs.stdenvAdapters.useMoldLinker pkgs.clangStdenv;
              };
            })
              {
                LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath ([ ]);
                buildInputs =
                  with pkgs;
                  [
                    nodejs
                    pnpm
                    svelte-language-server

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

                    binaryen
                    cmake
                    maturin
                    ninja
                    buf
                    protobuf_28
                    pkg-config
                    rust-analyzer
                    rust-toolchain
                  ]
                  ++ lib.optional stdenv.isDarwin [
                    darwin.apple_sdk.frameworks.Security
                    darwin.apple_sdk.frameworks.CoreFoundation
                    darwin.apple_sdk.frameworks.SystemConfiguration
                    libiconv
                  ];
              };
        };
    };

}
