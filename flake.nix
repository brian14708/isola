{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      fenix,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        rust-toolchain = (
          with fenix.packages.${system};
          combine [
            (stable.withComponents [
              "cargo"
              "clippy"
              "rustc"
              "rustfmt"
            ])
            targets.wasm32-wasip1.stable.rust-std
          ]
        );
        mkShell = pkgs.mkShell.override {
          stdenv =
            if pkgs.stdenv.isLinux then
              pkgs.stdenvAdapters.useMoldLinker pkgs.clangStdenv
            else
              pkgs.clangStdenv;
        };
      in
      {
        formatter = pkgs.nixfmt-rfc-style;
        devShells.default = mkShell {
          buildInputs =
            with pkgs;
            [
              nodejs
              pnpm
              svelte-language-server

              (python313.withPackages (ps: with ps; [ pip ]))

              cmake
              protobuf_28
              pkg-config
              gcc
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
      }
    );

}
