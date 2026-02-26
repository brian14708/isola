{
  stdenv,
  makeBinaryWrapper,
  server,
  python,
  js,
}:
stdenv.mkDerivation {
  pname = "isola";
  version = "0.2.0";

  dontUnpack = true;

  nativeBuildInputs = [ makeBinaryWrapper ];

  buildPhase = ''
    runHook preBuild

    # Copy the complete Python runtime output.
    mkdir -p app
    cp --no-preserve=mode -rL ${python}/. app/
    mkdir -p app/target/wasm32-wasip1/wasi-deps
    ln -s ../bin/python3.wasm app/target/python3.wasm
    ln -s ../../.. app/target/wasm32-wasip1/wasi-deps/usr

    # Install the JS runtime WASM.
    cp ${js}/bin/js.wasm app/bin/js.wasm
    ln -s ../bin/js.wasm app/target/js.wasm

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
      --set WASI_PYTHON_RUNTIME $out/share/isola

    # Run the build step to pre-initialize the VM
    cd $out/share/isola
    WASI_PYTHON_RUNTIME=$out/share/isola \
      $out/libexec/isola/isola-server build

    runHook postInstall
  '';

  meta = {
    mainProgram = "isola";
  };
}
