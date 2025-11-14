{
  stdenv,
  python314,
  pkg-config,
  wasipkgs,
}:
let
  inherit (wasipkgs) wasi-optimize-hook zlib sdk;
in
stdenv.mkDerivation rec {
  pname = "${python314.pname}-wasi";
  version = python314.version;
  src = python314.src;
  dontStrip = true;

  buildInputs = [ zlib ];

  passthru = {
    host = python314;
  };

  nativeBuildInputs = [
    pkg-config
    python314
    wasi-optimize-hook
  ];

  buildArch =
    if stdenv.isDarwin then
      if stdenv.isAarch64 then "arm64-apple-darwin" else "x86_64-apple-darwin"
    else if stdenv.isLinux then
      if stdenv.isAarch64 then "aarch64-unknown-linux-gnu" else "x86_64-unknown-linux-gnu"
    else
      builtins.throw "Unsupported platform for Python WASI build";

  configureFlags = [
    "--prefix=/"
    "--host=wasm32-wasi"
    "--build=${buildArch}"
    "--with-build-python=${python314}/bin/python3"
    "--enable-shared"
    "--disable-test-modules"
    "--with-tzpath=/lib/python3.14/site-packages/tzdata/zoneinfo"
    "--enable-big-digits=30"
    "--with-pymalloc"
  ];

  preConfigure = ''
    export CONFIG_SITE=$PWD/Tools/wasm/wasi/config.site-wasm32-wasi
    export CFLAGS="$CFLAGS -fPIC"
    export CC="${sdk}/bin/clang --sysroot=${sdk}/share/wasi-sysroot"
    export CXX="${sdk}/bin/clang++ --sysroot=${sdk}/share/wasi-sysroot"
    export AR=${sdk}/bin/llvm-ar
    export RANLIB=${sdk}/bin/ranlib
  '';

  buildPhase = ''
    runHook preBuild
    make build_all
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    DESTDIR=$out make install
    runHook postInstall
  '';

  postInstall = ''
    export _PYTHON_HOST_PLATFORM=wasi-wasm32
    export _PYTHON_SYSCONFIGDATA_NAME=_sysconfigdata__wasi_wasm32-wasi

    ${python314}/bin/python3 $PWD/Tools/wasm/emscripten/wasm_assets.py \
      --prefix $out \
      --output $out/lib/python314.zip

    touch $out/python-stub.c
    $CC $CFLAGS \
      -shared -o $out/lib/libpython3.14.so \
      $out/python-stub.c \
      -Wl,--whole-archive $out/lib/libpython3.14.a -Wl,--no-whole-archive \
      $(pkg-config --libs zlib) \
      $PWD/Modules/_hacl/libHacl_Hash_SHA1.a \
      $PWD/Modules/_hacl/libHacl_Hash_SHA2.a \
      $PWD/Modules/_hacl/libHacl_Hash_SHA3.a \
      $PWD/Modules/_hacl/libHacl_Hash_MD5.a \
      $PWD/Modules/_hacl/libHacl_Hash_BLAKE2.a \
      $PWD/Modules/_hacl/libHacl_HMAC.a \
      $PWD/Modules/_decimal/libmpdec/libmpdec.a \
      $PWD/Modules/expat/libexpat.a \
      ${sdk}/lib/clang/21/lib/wasm32-unknown-wasip1/libclang_rt.builtins.a \
      ${sdk}/share/wasi-sysroot/lib/wasm32-wasip1/libwasi-emulated-signal.so \
      ${sdk}/share/wasi-sysroot/lib/wasm32-wasip1/libwasi-emulated-process-clocks.so \
      ${sdk}/share/wasi-sysroot/lib/wasm32-wasip1/libwasi-emulated-getpid.so \
      ${sdk}/share/wasi-sysroot/lib/wasm32-wasip1/libdl.so \
      ${sdk}/share/wasi-sysroot/lib/wasm32-wasip1/libc.so
    rm $out/lib/libpython3.14.a $out/python-stub.c

    rm -rf $out/bin/
    rm -rf $out/lib/python3.14/config-3.14-wasm32-wasi/
    find $out/ -type f -name "*.a" -exec rm {} +
    find $out/ -type d -name "__pycache__" -exec rm -rf {} +
  '';
}
