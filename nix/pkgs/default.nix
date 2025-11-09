{
  pkgs,
  craneLib,
}:
let
  packages = pkgs.lib.packagesFromDirectoryRecursive {
    callPackage = pkgs.lib.callPackageWith (pkgs // { inherit craneLib; } // packages);
    directory = ./.;
  };
in
{
  inherit (packages) promptkit server python;
  default = packages.promptkit;
  oci = packages.container;
}
