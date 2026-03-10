{
  description = "Greens - a collection of small virtual devices";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-25.11";
    pre-commit-hooks.url = "github:cachix/pre-commit-hooks.nix";
    # To test locally:
    # nix build --override-input sel4-virt ../kmod-sel4-virt
    sel4-virt = {
      url = "github:tiiuae/kmod-sel4-virt/main";
      flake = false;
    };
  };

  outputs =
    { self, nixpkgs, ... }@inputs:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
      pkgsFor = forAllSystems (system: import nixpkgs { system = system; });
    in
    {
      checks = forAllSystems (system: {
        pre-commit-check = inputs.pre-commit-hooks.lib.${system}.run {
          src = ./.;
          hooks = {
            # lint shell scripts
            shellcheck.enable = true;
            # execute example shell from markdown
            mdsh.enable = true;
            # mixed line endings
            mixed-line-endings.enable = true;
            # check nix formatting
            nixpkgs-fmt.enable = true;
            # check rust formatting
            rustfmt = {
              enable = true;
              packageOverrides = {
                cargo = pkgsFor.${system}.cargo;
                rustfmt = pkgsFor.${system}.rustfmt;
              };
            };
            # check toml
            taplo.enable = true;
          };
        };
      });
      packages = forAllSystems (system: {
        default = pkgsFor.${system}.callPackage ./default.nix {
          pkgs = import nixpkgs { inherit system; };
          kmod = inputs.sel4-virt;
        };
      });
      devShells = forAllSystems (system: {
        default = pkgsFor.${system}.callPackage ./shell.nix {
          pkgs = import nixpkgs { inherit system; };
          preCommit = (self.checks.${system}.pre-commit-check);
          kmod = inputs.sel4-virt;
        };
      });
    };
}
