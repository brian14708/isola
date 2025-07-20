{ pkgs, ... }:
{
  projectRootFile = "flake.nix";
  programs = {
    nixfmt.enable = true;
    rustfmt = {
      enable = true;
      package = pkgs.rust-bin.nightly.latest.default;
    };
    ruff-format.enable = true;
    buf.enable = true;
    cmake-format.enable = true;
    clang-format.enable = true;
  };
  settings.formatter = {
    cmake-format = {
      includes = [
        "*.cmake"
        "*/CMakeLists.txt"
      ];
    };
    clang-format = {
      excludes = [
        "crates/c-api/include/promptkit.h"
      ];
    };
    ruff-format = {
      excludes = [
        "tests/rpc/stub/*"
      ];
    };
  };
}
