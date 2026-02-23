{ pkgs, ... }:
{
  projectRootFile = "flake.nix";
  programs = {
    nixfmt.enable = true;
    rustfmt = {
      enable = true;
      package = pkgs.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml;
    };
    ruff-format.enable = true;
    clang-format.enable = true;
    actionlint.enable = true;
    prettier.enable = true;
  };
}
