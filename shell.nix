{ pkgs, kmod, preCommit }:
let
  GREENS_ROOT = toString ./.;
  RUST_BACKTRACE = 1;
  CARGO_INSTALL_ROOT = "${GREENS_ROOT}/.cargo";
  SEL4_VIRT_HEADERS_PATH = "${kmod}/include/uapi";
  buildInputsFrom = inputs:
    (pkgs.lib.subtractLists inputs (pkgs.lib.flatten (pkgs.lib.catAttrs "buildInputs" inputs)));
in
pkgs.mkShell rec {
  inherit (preCommit) shellHook;
  inherit CARGO_INSTALL_ROOT;
  inherit RUST_BACKTRACE;
  inherit SEL4_VIRT_HEADERS_PATH;

  inputsFrom = [
    (pkgs.callPackage ./default.nix { inherit pkgs; })
  ];

  nativeBuildInputs = with pkgs; [
    cargo
    rust-analyzer
    rustfmt
    clippy
    gdb
    preCommit.enabledPackages
    cargo-hack
    cargo-deny
    cargo-outdated
    cargo-vet
    cargo-supply-chain
    just
  ];

  # shared libraries for running
  packages = buildInputsFrom inputsFrom;

  hardeningDisable = [ "fortify" ];
}
