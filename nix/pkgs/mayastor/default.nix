{ stdenv
, clang
, dockerTools
, lib
, libaio
, libiscsi
, libspdk
, libudev
, liburing
, llvmPackages
, makeRustPlatform
, numactl
, openssl
, pkg-config
, protobuf
, release ? true
, sources
, utillinux
}:
let
  channel = import ../../lib/rust.nix { inherit sources; };
  rustPlatform = makeRustPlatform {
    rustc = channel.stable.rust;
    cargo = channel.stable.cargo;
  };

  whitelistSource = src: allowedPrefixes:
    builtins.filterSource
      (path: type:
        lib.any
          (allowedPrefix:
            lib.hasPrefix (toString (src + "/${allowedPrefix}")) path)
          allowedPrefixes)
      src;
in
rustPlatform.buildRustPackage rec {
  name = "mayastor";
  #cargoSha256 = "1583vkhr2qfjivq79c3kdfyfmjw5jwivvpd7yhd7ljvk9xh29q45";
  cargoSha256 = "0m3j4g5s0cak31zl4sa0mb18p2r307qdv6sbj1xnpm7yc6icg1fw";
  version = sources.mayastor.branch;
  src = if release then sources.mayastor else
  whitelistSource ../../../. [
    "Cargo.lock"
    "Cargo.toml"
    "cli"
    "csi"
    "devinfo"
    "jsonrpc"
    "mayastor"
    "nvmeadm"
    "rpc"
    "spdk-sys"
    "sysfs"
  ];

  LIBCLANG_PATH = "${llvmPackages.libclang}/lib";

  PROTOC = "${protobuf}/bin/protoc";
  PROTOC_INCLUDE = "${protobuf}/include";
  SPDK_PATH = "${libspdk}";
  nativeBuildInputs = [
    clang
    pkg-config
  ];

  buildInputs = [
    llvmPackages.libclang
    protobuf
    libaio
    libiscsi.lib
    libspdk
    libudev
    liburing
    numactl
    openssl
    utillinux
  ];

  buildType = if release then "release" else "debug";
  verifyCargoDeps = false;

  doCheck = false;
  meta = { platforms = stdenv.lib.platforms.linux; };
}
