{ pkgs, ... }:
{
  projectRootFile = "flake.nix";
  programs = {
    nixfmt.enable = true;
    rustfmt = {
      enable = true;
      package = pkgs.rust-bin.stable.latest.default;
    };
    ruff-format.enable = true;
    buf.enable = true;
    clang-format.enable = true;
  };
  settings.formatter = {
    clang-format = {
      excludes = [
        "crates/c-api/include/promptkit.h"
      ];
    };
    ruff-format = {
      excludes = [
        "tests/rpc/src/stub/*"
      ];
    };
  };
}
