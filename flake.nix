{
  description = "rerun-operator — Kubernetes operator for Rerun (rerun.io) dashboards";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # Cargo cannot authenticate private git dependencies during Nix evaluation.
    # Fetch the shared source through the authenticated GitHub flake fetcher,
    # then patch the generated Cargo source to use this pinned tree.
    hephaestus = {
      url = "github:casazza-info/hephaestus/e2868ee84f2d4f491edf1adbdbd163f4e6c83d1a";
      flake = false;
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      rust-overlay,
      hephaestus,
    }:
    let
      expectedHephaestusRev = hephaestus.rev;
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

          cleanSrc = craneLib.cleanCargoSource ./.;
          src = pkgs.runCommand "rerun-operator-source" { } ''
            mkdir -p "$out"
            cp -R ${cleanSrc}/. "$out/"
            chmod -R u+w "$out"
            mkdir -p "$out/vendor/hephaestus"
            cp -R ${hephaestus}/. "$out/vendor/hephaestus/"
            expected_source='git+https://github.com/casazza-info/hephaestus?branch=main#${expectedHephaestusRev}'
            test "$(grep -Fc 'name = "hephaestus-operator-lib"' "$out/Cargo.lock")" -eq 1
            test "$(grep -Fxc "source = \"$expected_source\"" "$out/Cargo.lock")" -eq 1
            cat >> "$out/Cargo.toml" <<'EOF'

            [patch."https://github.com/casazza-info/hephaestus"]
            hephaestus-operator-lib = { path = "vendor/hephaestus/crates/hephaestus-operator-lib" }
            EOF
            grep -Fv "source = \"$expected_source\"" "$out/Cargo.lock" > "$out/Cargo.lock.tmp"
            mv "$out/Cargo.lock.tmp" "$out/Cargo.lock"
            if grep -Fq 'source = "git+https://github.com/casazza-info/hephaestus' "$out/Cargo.lock"; then
              echo "unexpected hephaestus git source after pinned patch" >&2
              exit 1
            fi
          '';

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

      # The x86_64-linux Buildbot gate builds the same operator derivation
      # exposed as the package; no synthetic test derivation is introduced.
      checks.x86_64-linux.default = self.packages.x86_64-linux.rerun-operator;

      # Hydra's x86 artifact reuses the operator package derivation; package
      # outputs for the other declared systems remain unchanged.
      hydraJobs.x86_64-linux.rerun-operator = self.packages.x86_64-linux.rerun-operator;

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
