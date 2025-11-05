{ pkgs, kmod }:
# could also use
# let
#   rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
pkgs.rustPlatform.buildRustPackage {
  pname = "greens";
  version = "0.1";
  cargoLock.lockFile = ./Cargo.lock;

  cargoLock.outputHashes = {
    "virtio-bindings-0.2.3" =
      "sha256-lwqWWn6X0KTo3vWY+ObNwBe8tRVNQL/k26zH6A0xPUM=";
  };

  src = pkgs.lib.cleanSource ./.;
  checkType = "debug";
  nativeBuildInputs = [
    pkgs.rustc
    pkgs.cargo
    pkgs.rustPlatform.bindgenHook
  ];
  SEL4_VIRT_HEADERS_PATH = "${kmod}/include/uapi";
}
