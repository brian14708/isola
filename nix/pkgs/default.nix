{
  pkgs,
  crane,
}:
let
  packages = pkgs.lib.packagesFromDirectoryRecursive {
    callPackage = pkgs.lib.callPackageWith (pkgs // { inherit crane; } // packages);
    directory = ./.;
  };
in
{
  inherit (packages) python js;
  default = packages.isola;
  oci = packages.container;
}
