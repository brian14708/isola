{
  stdenv,
  wasipkgs,
  fetchPypi,
  nukeReferences,
}:
let
  inherit (wasipkgs) sdk python;
  pythonVersion = python.host.pythonVersion;
  pythonSitePackages = "lib/python${pythonVersion}/site-packages";

  bundlePackages =
    (builtins.map (pkg: "${pkg}/${pythonSitePackages}") (
      with python.host.pkgs;
      [
        setuptools
        typing-extensions
        annotated-types
        typing-inspection
        xmltodict
        pydantic
      ]
    ))
    ++ (builtins.map toString [
      (fetchPypi {
        pname = "duron";
        version = "0.0.3";
        format = "wheel";
        python = "py3";
        dist = "py3";
        hash = "sha256-6clLYJzGSNPzVZESBaAt1lYe16Q6zNL+buMvgobKKo4=";
      })
    ]);

  outPackages =
    (with wasipkgs.pythonPackages; [
      numpy
      pillow
      pydantic-core
    ])
    ++ (with python.host.pkgs; [
      tzdata
    ]);

  promptkit-py = ../../../crates/python/bundled;
in
stdenv.mkDerivation {
  pname = "wasi-python-bundle";
  inherit (python) version;
  dontUnpack = true;
  dontStrip = true;

  nativeBuildInputs = [
    python.host
    nukeReferences
  ];
  buildInputs = [
    python
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
      ${builtins.concatStringsSep " " bundlePackages} \
      $TMPDIR/py

    runHook postBuild
  '';

  installPhase =
    let
      outPackagesList = builtins.concatStringsSep " " (builtins.map toString outPackages);
    in
    ''
      runHook preInstall

      mkdir -p $dev
      cp --no-preserve=mode -rL ${python}/lib ${python}/include $dev
      for pkg in ${outPackagesList}; do
        cp --no-preserve=mode -rL $pkg/${pythonSitePackages}/* $dev/${pythonSitePackages}/
      done
      cp --no-preserve=mode -rL ${sdk}/share/wasi-sysroot/lib/wasm32-wasip1/*.so $dev/lib

      mkdir -p $out/${pythonSitePackages} $out/lib/python${pythonVersion}/lib-dynload
      for pkg in ${outPackagesList}; do
        cp --no-preserve=mode -rL $pkg/${pythonSitePackages}/* $out/${pythonSitePackages}/
      done
      touch $out/lib/python${pythonVersion}/lib-dynload/.empty

      cp $TMPDIR/bundle.zip $TMPDIR/bundle-src.zip ${python}/lib/python*.zip $out/lib/

      find $out/ -type f -name "*.so" -exec truncate -s 0 {} \;
      find $out/ -type d -name "__pycache__" -exec rm -rf {} +

      ${python.host}/bin/python3 -m compileall $out/
      find $out/ -type f -print -exec nuke-refs '{}' +

      runHook postInstall
    '';
}
