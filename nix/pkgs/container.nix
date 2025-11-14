{
  dockerTools,
  promptkit,
  coreutils,
  bash,
  cacert,
}:

dockerTools.buildLayeredImage {
  name = "promptkit";
  tag = "latest";

  contents = [
    promptkit
    coreutils
    bash
    cacert
  ];

  config = {
    Cmd = [ "${promptkit}/bin/promptkit" ];
    WorkingDir = "${promptkit}/share/promptkit";
    Env = [
      "WASI_PYTHON_RUNTIME=${promptkit}/share/promptkit/target/wasm32-wasip1/wasi-deps/usr"
      "SSL_CERT_FILE=${cacert}/etc/ssl/certs/ca-bundle.crt"
    ];
    ExposedPorts = {
      "3000/tcp" = { };
    };
    Labels = {
      "org.opencontainers.image.source" = "https://github.com/brian14708/promptkit";
    };
  };
}
