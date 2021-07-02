{ config, pkgs, ... }:

{

  imports = [
    <nixpkgs/nixos/modules/profiles/qemu-guest.nix>
    <nixpkgs/nixos/modules/virtualisation/qemu-vm.nix>
  ];

  boot = {
    kernelPackages = pkgs.linuxPackages_latest;
    kernelParams = [
      "mitigations=off"
    ];

    kernelModules = [ "nvme-tcp" ];
    kernel.sysctl = { "vm.nr_hugepages" = 512; };
  };

  environment.systemPackages = with pkgs; [
    fio
    zsh
    nvme-cli
  ];

  virtualisation = {
    memorySize = 2048; # MB
    cores = 2;
  };

  users.users.root.password = "nixos";

  time.timeZone = "UTC";

  security.sudo.wheelNeedsPassword = false;

  users.users.jenkins = {
    isNormalUser = true;
    extraGroups = [ "wheel" ];
    password = "";
  };

  networking.networkmanager.enable = false;
  networking.hostName = "pytest_vm";
  systemd.coredump.enable = true;
  services = {
    openssh = {
      enable = true;
    };
  };
}

