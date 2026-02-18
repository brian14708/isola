{
  stdenv,
  makeBinaryWrapper,
  server,
  python,
}:
let
  inherit (python) bundle;
in
stdenv.mkDerivation {
  pname = "isola";
  version = "0.1.0";

  dontUnpack = true;

  nativeBuildInputs = [ makeBinaryWrapper ];

  buildPhase = ''
    runHook preBuild

    # Create the application directory structure
    mkdir -p app/target
    mkdir -p app/target/wasm32-wasip1/wasi-deps/usr

    # Copy all components
    cp ${python}/lib/isola_python.wasm app/target/isola_python.wasm
    cp -r ${bundle}/* app/target/wasm32-wasip1/wasi-deps/usr/

    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall

    # Install the complete application directory
    mkdir -p $out/share/isola
    cp -r app/* $out/share/isola/

    # Install the server binary
    mkdir -p $out/libexec/isola
    cp ${server}/bin/isola-server $out/libexec/isola/isola-server

    # Create wrapper script that runs from the correct directory
    mkdir -p $out/bin
    makeBinaryWrapper $out/libexec/isola/isola-server $out/bin/isola \
      --chdir $out/share/isola \
      --set WASI_PYTHON_RUNTIME $out/share/isola/target/wasm32-wasip1/wasi-deps/usr

    # Run the build step to pre-initialize the VM
    cd $out/share/isola
    WASI_PYTHON_RUNTIME=$out/share/isola/target/wasm32-wasip1/wasi-deps/usr \
      $out/libexec/isola/isola-server build

    runHook postInstall
  '';

  meta = {
    mainProgram = "isola";
  };
}
