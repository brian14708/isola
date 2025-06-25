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
          overlays = [ rust-overlay.overlays.default ];
          config = {
            allowUnfree = true;
            android_sdk.accept_license = true;
          };
        };

        rustToolchain = (
          p:
          p.rust-bin.stable.latest.default.override {
            targets = [ "wasm32-wasip1" ];
          }
        );
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;
        craneLibFull = (crane.mkLib pkgs).overrideToolchain (
          p:
          p.rust-bin.stable.latest.default.override {
            extensions = [ "rust-src" ];
            targets = [
              "wasm32-wasip1"
              "aarch64-linux-android"
            ];
          }
        );

        treefmtEval = treefmt-nix.lib.evalModule pkgs {
          projectRootFile = "flake.nix";
          programs = {
            nixfmt.enable = true;
            gofumpt.enable = true;
            rustfmt = {
              enable = true;
              package = rustToolchain pkgs;
            };
            buf.enable = true;
          };
        };

        deps = with pkgs; [
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
          protobuf
          pkg-config
        ];

        mkShell = pkgs.mkShell.override {
          stdenv = pkgs.stdenvAdapters.useMoldLinker pkgs.clangStdenv;
        };
      in
      {
        devShells = {
          default = (craneLib.devShell.override { inherit mkShell; }) { buildInputs = deps; };

          full = (craneLibFull.devShell.override { inherit mkShell; }) (
            let
              androidComposition = pkgs.androidenv.composeAndroidPackages {
                abiVersions = [ "arm64-v8a" ];
                includeNDK = true;
                platformVersions = [ "30" ];
              };
            in
            rec {
              buildInputs = deps ++ [ pkgs.android-tools ];

              ANDROID_SDK_ROOT = "${androidComposition.androidsdk}/libexec/android-sdk";
              ANDROID_NDK_ROOT = "${ANDROID_SDK_ROOT}/ndk-bundle";
              CC_aarch64_linux_android = "${ANDROID_NDK_ROOT}/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android30-clang";
              CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = "${ANDROID_NDK_ROOT}/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android30-clang";
            }
          );
        };

        formatter = treefmtEval.config.build.wrapper;
        checks.formatting = treefmtEval.config.build.check self;
      }
    );

}
