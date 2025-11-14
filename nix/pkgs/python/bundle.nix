{
  stdenv,
  wasipkgs,
  fetchPypi,
  nukeReferences,
}:
let
  inherit (wasipkgs) sdk python;
  inherit (wasipkgs.pythonPackages)
    numpy
    pillow
    pydantic-core
    ;

  setuptools = fetchPypi {
    pname = "setuptools";
    version = "80.9.0";
    format = "wheel";
    python = "py3";
    dist = "py3";
    hash = "sha256-Bi00IirRPgzDEqTALXPwWehqSsv73qj492soyZ8waSI=";
  };

  typing-extensions = fetchPypi {
    pname = "typing_extensions";
    version = "4.15.0";
    format = "wheel";
    python = "py3";
    dist = "py3";
    hash = "sha256-8PoZxoRXWKsIB0oM+ot67LccmZynPWKIO8JcwBjE5Ug=";
  };

  annotated-types = fetchPypi {
    pname = "annotated_types";
    version = "0.7.0";
    format = "wheel";
    python = "py3";
    dist = "py3";
    hash = "sha256-HwLotDqPu8Pz4NTw9L/IExvLTuvohJuOXHc/OhxYKlM=";
  };

  typing-inspection = fetchPypi {
    pname = "typing_inspection";
    version = "0.4.2";
    format = "wheel";
    python = "py3";
    dist = "py3";
    hash = "sha256-TtHKy9wpjCIPG9JJ7VKHyqFvNNRO9OnD0MutW1IVRec=";
  };

  xmltodict = fetchPypi {
    pname = "xmltodict";
    version = "1.0.2";
    format = "wheel";
    python = "py3";
    dist = "py3";
    hash = "sha256-YtD92w3LyfZCdF2Lv02B/RfW367FoVtcGHYwCq2Srw0=";
  };

  pydantic = fetchPypi {
    pname = "pydantic";
    version = "2.12.3";
    format = "wheel";
    python = "py3";
    dist = "py3";
    hash = "sha256-aYZFSoVLw7xuVEPhNp4Go6RWr50zntpFUQ9RfZ6lxr8=";
  };

  duron = fetchPypi {
    pname = "duron";
    version = "0.0.3";
    format = "wheel";
    python = "py3";
    dist = "py3";
    hash = "sha256-6clLYJzGSNPzVZESBaAt1lYe16Q6zNL+buMvgobKKo4=";
  };

  promptkit-py = ../../../crates/python/bundled;
in
stdenv.mkDerivation {
  pname = "wasi-python-bundle";
  version = python.version;
  dontUnpack = true;
  dontStrip = true;

  nativeBuildInputs = [
    python.host
    nukeReferences
  ];
  buildInputs = [
    python
    numpy
    pillow
    pydantic-core
    sdk
  ];

  outputs = [
    "out"
    "dev"
  ];

  buildPhase = ''
    runHook preBuild

    # Run bundle.py similar to xtask/src/main.rs
    mkdir -p $TMPDIR/bundle

    cp -r ${promptkit-py} $TMPDIR/py
    chmod -R +w $TMPDIR/py

    ${python.host}/bin/python3 ${./bundle.py} $TMPDIR/bundle \
      ${setuptools} \
      ${typing-extensions} \
      ${annotated-types} \
      ${typing-inspection} \
      ${xmltodict} \
      ${pydantic} \
      ${duron} \
      $TMPDIR/py

    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall

    mkdir -p $dev
    cp --no-preserve=mode -rL ${python}/lib ${python}/include $dev
    cp --no-preserve=mode -rL ${numpy}/lib/python3.14/site-packages/* $dev/lib/python3.14/site-packages/
    cp --no-preserve=mode -rL ${pillow}/lib/python3.14/site-packages/* $dev/lib/python3.14/site-packages/
    cp --no-preserve=mode -rL ${pydantic-core}/lib/python3.14/site-packages/* $dev/lib/python3.14/site-packages/
    cp --no-preserve=mode -rL ${sdk}/share/wasi-sysroot/lib/wasm32-wasip1/*.so $dev/lib

    mkdir -p $out/lib/python3.14/site-packages $out/lib/python3.14/lib-dynload
    cp --no-preserve=mode -rL ${numpy}/lib/python3.14/site-packages/* $out/lib/python3.14/site-packages/
    cp --no-preserve=mode -rL ${pillow}/lib/python3.14/site-packages/* $out/lib/python3.14/site-packages/
    cp --no-preserve=mode -rL ${pydantic-core}/lib/python3.14/site-packages/* $out/lib/python3.14/site-packages/
    cp --no-preserve=mode -rL ${python.host.pkgs.tzdata}/lib/python3.14/site-packages/* $out/lib/python3.14/site-packages/
    touch $out/lib/python3.14/lib-dynload/.empty

    cp $TMPDIR/bundle.zip $TMPDIR/bundle-src.zip ${python}/lib/python314.zip $out/lib/

    find $out/ -type f -name "*.so" -exec truncate -s 0 {} \;
    find $out/ -type d -name "__pycache__" -exec rm -rf {} +

    ${python.host}/bin/python3 -m compileall $out/
    find $out/ -type f -print -exec nuke-refs '{}' +

    runHook postInstall
  '';
}
