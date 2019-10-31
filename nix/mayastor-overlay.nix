self: super: {
  libiscsi = super.callPackage ./pkgs/libiscsi {};
  nvme-cli = super.callPackage ./pkgs/nvme-cli {};
  libspdk = super.callPackage ./pkgs/libspdk { enableDebug = false; };
  mayastor = (super.callPackage ./pkgs/mayastor {}).mayastor;
  mayastorImage = (super.callPackage ./pkgs/mayastor {}).mayastorImage;
  mayastorCSIImage = (super.callPackage ./pkgs/mayastor {}).mayastorCSIImage;
  k9s = super.callPackage ./pkgs/k9s {};
  stern = super.callPackage ./pkgs/stern {};
  node-moac = (import ./../csi/moac { pkgs = super; }).package;
}
