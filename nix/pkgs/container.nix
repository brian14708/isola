{
  lib,
  dockerTools,
  isola,
}:
dockerTools.streamLayeredImage {
  name = "isola";
  tag = "latest";

  contents = [
    dockerTools.caCertificates
  ];

  config = {
    Cmd = [ (lib.getExe isola) ];
    ExposedPorts = {
      "3000/tcp" = { };
    };
    Labels = {
      "org.opencontainers.image.source" = "https://github.com/brian14708/isola";
    };
  };
}
