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
      in
      {
        formatter = pkgs.nixfmt-rfc-style;
        devShells.default =
          (pkgs.buildFHSUserEnv {
            name = "devshell";
            targetPkgs =
              pkgs:
              with pkgs;
              [
                nodejs
                pnpm
                svelte-language-server

                python313
                uv

                cmake
                buf
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
          }).env;
      }
    );

}
