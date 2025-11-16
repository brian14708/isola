{
  dockerTools,
  promptkit,
}:
dockerTools.buildLayeredImage {
  name = "promptkit";
  tag = "latest";

  contents = [
    promptkit
  ];

  config = {
    Cmd = [ "${promptkit}/bin/promptkit" ];
    WorkingDir = "${promptkit}/share/promptkit";
    ExposedPorts = {
      "3000/tcp" = { };
    };
    Labels = {
      "org.opencontainers.image.source" = "https://github.com/brian14708/promptkit";
    };
  };
}
