{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
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
      treefmt-nix,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [
            rust-overlay.overlays.default
            (final: prev: {
              wasipkgs = prev.lib.packagesFromDirectoryRecursive {
                callPackage = prev.lib.callPackageWith final;
                directory = ./nix/wasipkgs;
              };
            })
          ];
        };

        treefmtEval = treefmt-nix.lib.evalModule pkgs ./nix/treefmt.nix;
      in
      {
        packages = import ./nix/pkgs { inherit pkgs crane; };
        devShells = {
          default = import ./nix/shell.nix {
            inherit pkgs crane;
            packages = self.packages.${system};
          };
        };
        formatter = treefmtEval.config.build.wrapper;
        checks.formatting = treefmtEval.config.build.check self;
      }
    );

  nixConfig = {
    extra-substituters = [
      "https://promptkit.cachix.org"
    ];
    extra-trusted-public-keys = [
      "promptkit.cachix.org-1:IHR3VcUtLnWIqLkKk8UbSe0lMYW0C9tNVbqN5FYUYrQ="
    ];
  };
}
