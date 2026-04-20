{
  description = "rerun-operator — Kubernetes operator for Rerun (rerun.io) dashboards";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      rust-overlay,
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    in
    {
      kubenixModules.rerun-operator = import ./nix/kubenix-module.nix;

      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ (import rust-overlay) ];
          };

          rustToolchain = pkgs.rust-bin.stable.latest.default.override {
            extensions = [
              "rust-src"
              "rust-analyzer"
              "clippy"
            ];
          };
          craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

          src = craneLib.cleanCargoSource ./.;

          commonArgs = {
            inherit src;
            strictDeps = true;
            nativeBuildInputs = with pkgs; [ pkg-config ];
            buildInputs = with pkgs; [ openssl ];
          };

          cargoArtifacts = craneLib.buildDepsOnly commonArgs;

          rerun-operator = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoExtraArgs = "-p rerun-operator";
            }
          );

          rerun-operator-image = pkgs.dockerTools.buildLayeredImage {
            name = "ghcr.io/casazza-info/rerun-operator";
            tag = "latest";
            contents = [
              rerun-operator
              pkgs.cacert
            ];
            config = {
              Entrypoint = [ "${rerun-operator}/bin/rerun-operator" ];
              Env = [
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
                "RUST_LOG=info,rerun_operator=debug"
              ];
            };
          };
        in
        {
          inherit rerun-operator rerun-operator-image;
          default = rerun-operator;
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ (import rust-overlay) ];
          };
          rustToolchain = pkgs.rust-bin.stable.latest.default.override {
            extensions = [
              "rust-src"
              "rust-analyzer"
              "clippy"
            ];
          };
        in
        {
          default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [
              rustToolchain
              pkg-config
              openssl
              cargo-watch
              kubectl
              kubernetes-helm
            ];
          };
        }
      );
    };
}
