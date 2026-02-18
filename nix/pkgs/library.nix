{
  lib,
  pkgs,
  crane,
  python,
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
      (craneLib.fileset.commonCargoSources ../../crates/c-api)
      (craneLib.fileset.commonCargoSources ../../crates/c-api-export)
    ];
  };
  src = pkgs.runCommand "isola-c-api-src" { } ''
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

        make_dummy_crate "crates/python" "isola-python"
        make_dummy_crate "crates/server" "isola-server"
        make_dummy_crate "crates/xtask" "xtask"
  '';
in
craneLib.buildPackage {
  pname = "isola-c-api";
  inherit src;
  strictDeps = true;

  CARGO_PROFILE = "release-lto";
  cargoExtraArgs = "-p isola-c-api-export";
  installPhase = ''
    runHook preInstall

    mkdir -p $out/lib -p $out/share/isola

    cp target/release-lto/libisola.* $out/lib/
    rm $out/lib/libisola.d
    runHook postInstall

    cp -r ${../../crates/c-api/include} $out/include
    cp -r ${python.bundle} $out/share/isola/python
    cp ${python}/lib/isola_python.wasm $out/share/isola/python.wasm
  '';
}
