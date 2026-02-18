{
  lib,
  pkgs,
  crane,
}:
let
  craneLib = (crane.mkLib pkgs).overrideToolchain (
    p: p.rust-bin.fromRustupToolchainFile ../../rust-toolchain.toml
  );
  baseSrc = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../wit
      ../../Cargo.lock
      ../../Cargo.toml
      (craneLib.fileset.commonCargoSources ../../crates/isola)
      (craneLib.fileset.commonCargoSources ../../crates/server)
    ];
  };
  src = pkgs.runCommand "isola-server-src" { } ''
        mkdir -p "$out"
        cp -r --no-preserve=mode ${baseSrc}/. "$out"

        make_dummy_crate() {
          local crate_dir="$1"
          local crate_name="$2"

          mkdir -p "$out/$crate_dir/src"
          cat > "$out/$crate_dir/Cargo.toml" <<EOF
    [package]
    name = "$crate_name"
    version = "0.0.0"
    edition = "2024"

    [lib]
    path = "src/lib.rs"
    EOF
          cat > "$out/$crate_dir/src/lib.rs" <<EOF
    pub fn stub() {}
    EOF
        }

        make_dummy_crate "crates/c-api" "isola-c-api"
        make_dummy_crate "crates/c-api-export" "isola-c-api-export"
        make_dummy_crate "crates/python" "isola-python"
        make_dummy_crate "crates/xtask" "xtask"
  '';
in
craneLib.buildPackage {
  pname = "isola-server";
  inherit src;
  strictDeps = true;

  CARGO_PROFILE = "release-lto";
  cargoExtraArgs = "-p isola-server";
  installPhase = ''
    runHook preInstall

    install -Dm755 target/release-lto/isola-server $out/bin/isola-server

    runHook postInstall
  '';
}
