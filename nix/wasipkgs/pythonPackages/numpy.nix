{
  stdenv,
  ninja,
  wasipkgs,
}:
let
  inherit (wasipkgs) wasi-optimize-hook sdk python;
  host = python.host;
in
stdenv.mkDerivation {
  pname = "${host.pkgs.numpy.pname}-wasi";
  version = host.pkgs.numpy.version;
  src = host.pkgs.numpy.src;
  dontStrip = true;

  nativeBuildInputs = [
    (host.withPackages (p: with p; [ cython ]))
    sdk
    ninja
    wasi-optimize-hook
  ];

  buildInputs = [ python ];

  patches = [ ./numpy.patch ];

  configurePhase = ''
    runHook preConfigure

    # Create Meson cross-compilation configuration
    cat > cross.cfg << EOF
    [constants]
    wasi_sdk_path = '${sdk}'
    wasi_sysroot = wasi_sdk_path / 'share' / 'wasi-sysroot'
    cross_prefix = wasi_sysroot / 'usr' / 'local'
    wasi_target = 'wasm32-wasip2'
    wasi_args = ['--sysroot=' + wasi_sysroot, '--target=' + wasi_target, '-D__EMSCRIPTEN__=1', '-DNPY_NO_SIGNAL']
    wasi_link_args = ['-L${python}/lib', '-L$PWD', '-lpython3.14']

    [binaries]
    c = [wasi_sdk_path + '/bin/clang', '-I${python}/include/python3.14']
    cpp = [wasi_sdk_path + '/bin/clang++', '-I${python}/include/python3.14', '-fno-exceptions']
    asm = wasi_sdk_path + '/bin/clang'
    ar = wasi_sdk_path + '/bin/llvm-ar'
    c_ld = wasi_sdk_path + '/bin/wasm-ld'
    cpp_ld = wasi_sdk_path + '/bin/wasm-ld'
    nm = wasi_sdk_path + '/bin/llvm-nm'
    ranlib = wasi_sdk_path + '/bin/llvm-ranlib'
    strip = wasi_sdk_path + '/bin/llvm-strip'
    objcopy = wasi_sdk_path + '/bin/llvm-objcopy'
    objdump = wasi_sdk_path + '/bin/llvm-objdump'

    [built-in options]
    c_args = wasi_args
    c_link_args = wasi_link_args
    cpp_args = wasi_args
    cpp_link_args = wasi_link_args

    [host_machine]
    cpu_family = 'wasm32'
    cpu = 'wasm32'
    system = 'wasi'
    endian = 'little'

    [properties]
    longdouble_format = 'IEEE_DOUBLE_LE'
    EOF

    export PYTHONPATH=${python}/lib/python3.14
    export _PYTHON_SYSCONFIGDATA_NAME=_sysconfigdata__wasi_wasm32-wasi

    touch python-stub.c
    ${sdk}/bin/clang --sysroot=${sdk}/share/wasi-sysroot \
      -c python-stub.c -o python-stub.o
    ${sdk}/bin/llvm-ar rcs libpython3.14.a python-stub.o

    python3 vendored-meson/meson/meson.py setup \
      --prefix $out \
      --cross-file=cross.cfg \
      --buildtype release \
      build

    runHook postConfigure
  '';

  buildPhase = ''
    runHook preBuild
    python3 vendored-meson/meson/meson.py compile -C build
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    python3 vendored-meson/meson/meson.py install \
      -C build \
      --no-rebuild \
      --tags=runtime,python-runtime,devel

    find $out/ -type f -name "*.a" -exec rm {} +
    find $out/ -type d -name "__pycache__" -exec rm -rf {} +
    runHook postInstall
  '';
}
