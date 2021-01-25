self: super: {
  libiscsi = super.callPackage ./pkgs/libiscsi { };
  nvmet-cli = super.callPackage ./pkgs/nvmet-cli { };
  libspdk = (super.callPackage ./pkgs/libspdk { }).release;
  libspdk-dev = (super.callPackage ./pkgs/libspdk { }).debug;
  mayastor = (super.callPackage ./pkgs/mayastor { }).release;
  convco = (super.callPackage ./pkgs/convco { });
  mayastor-dev = (super.callPackage ./pkgs/mayastor { }).debug;
  mayastor-adhoc = (super.callPackage ./pkgs/mayastor { }).adhoc;
  moac = (import ./../csi/moac { pkgs = super; }).package;
  images = super.callPackage ./pkgs/images { };

  ms-buildenv = super.callPackage ./pkgs/ms-buildenv { };
  mkContainerEnv = super.callPackage ./lib/mkContainerEnv.nix { };
}
