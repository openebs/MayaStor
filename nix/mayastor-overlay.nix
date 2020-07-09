self: super: {

  libiscsi = super.callPackage ./pkgs/libiscsi { };
  liburing = super.callPackage ./pkgs/liburing { };
  nvmet-cli = super.callPackage ./pkgs/nvmet-cli { };
  libspdk = super.callPackage ./pkgs/libspdk { };
  mayastor = super.callPackage ./pkgs/mayastor { };
  images = super.callPackage ./pkgs/images { };


  ms-buildenv = super.callPackage ./pkgs/ms-buildenv { };
  mkContainerEnv = super.callPackage ./lib/mkContainerEnv.nix { };


  node-moac = (import ./../csi/moac { pkgs = super; }).package;
  node-moacImage = (import ./../csi/moac { pkgs = super; }).buildImage;
  nodePackages = (import ./pkgs/nodePackages { pkgs = super; });

}
