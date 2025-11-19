{
  lib,
  dockerTools,
  promptkit,
}:
dockerTools.buildLayeredImage {
  name = "promptkit";
  tag = "latest";

  contents = [
    dockerTools.caCertificates
  ];

  config = {
    Cmd = [ (lib.getExe promptkit) ];
    ExposedPorts = {
      "3000/tcp" = { };
    };
    Labels = {
      "org.opencontainers.image.source" = "https://github.com/brian14708/promptkit";
    };
  };
}
