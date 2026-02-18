{
  stdenv,
  makeBinaryWrapper,
  server,
  python,
  ui,
}:
let
  inherit (python) bundle;
in
stdenv.mkDerivation {
  pname = "promptkit";
  version = "0.1.0";

  dontUnpack = true;

  nativeBuildInputs = [ makeBinaryWrapper ];

  buildPhase = ''
    runHook preBuild

    # Create the application directory structure
    mkdir -p app/target
    mkdir -p app/ui/dist
    mkdir -p app/target/wasm32-wasip1/wasi-deps/usr

    # Copy all components
    cp ${python}/lib/promptkit_python.wasm app/target/promptkit_python.wasm
    cp -r ${ui}/* app/ui/dist/
    cp -r ${bundle}/* app/target/wasm32-wasip1/wasi-deps/usr/

    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall

    # Install the complete application directory
    mkdir -p $out/share/promptkit
    cp -r app/* $out/share/promptkit/

    # Install the server binary
    mkdir -p $out/libexec/promptkit
    cp ${server}/bin/isola-server $out/libexec/promptkit/isola-server

    # Create wrapper script that runs from the correct directory
    mkdir -p $out/bin
    makeBinaryWrapper $out/libexec/promptkit/isola-server $out/bin/promptkit \
      --chdir $out/share/promptkit \
      --set WASI_PYTHON_RUNTIME $out/share/promptkit/target/wasm32-wasip1/wasi-deps/usr

    # Run the build step to pre-initialize the VM
    cd $out/share/promptkit
    WASI_PYTHON_RUNTIME=$out/share/promptkit/target/wasm32-wasip1/wasi-deps/usr \
      $out/libexec/promptkit/isola-server build

    runHook postInstall
  '';

  meta = {
    mainProgram = "promptkit";
  };
}
