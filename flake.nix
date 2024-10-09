{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
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
        mkShell = pkgs.mkShell.override { stdenv = pkgs.stdenvAdapters.useMoldLinker pkgs.stdenv; };
      in
      {
        devShells.default = mkShell {
          packages = with pkgs; [
            nixfmt-rfc-style

            nodejs
            pnpm
            svelte-language-server

            (python313.withPackages (ps: with ps; [ pip ]))

            cmake
            protobuf_28
            gcc
            wizer
            binaryen
            rust-analyzer
            rust-toolchain
          ];
        };
      }
    );

}
