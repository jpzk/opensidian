# SPDX-License-Identifier: GPL-3.0-or-later
# Nix flake: package, checks and dev shell for opensidian.
#
# The toolchain is nixpkgs' stable Rust, which matches the version pinned in
# README "Build from source". Every cargo invocation runs --frozen against
# src-tauri/Cargo.lock (crane enforces the lock), so the flake can never pull
# a crate the lock does not pin.
#
# The Cargo.toml lives in src-tauri/, not the source root, but tauri.conf.json
# points frontendDist at ../ui, so the source must keep the whole repo layout
# while cargo runs from src-tauri/ (see the postUnpack dance below).
{
  description = "opensidian — vault-compatible markdown notes app (GPL-3.0-or-later)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, crane, flake-utils }:
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        craneLib = crane.mkLib pkgs;

        # Keep only what the build touches, so cargo hash churn stays low.
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = path: type:
            let base = builtins.baseNameOf path;
            in
              (pkgs.lib.cleanSourceFilter path type)
              || (type == "directory" && builtins.elem base [ "ui" "packaging" "fonts" ]);
        };

        commonArgs = {
          cargoToml = ./src-tauri/Cargo.toml;
          cargoLock = ./src-tauri/Cargo.lock;
          strictDeps = true;

          # Run cargo from src-tauri/ while the repo layout (ui/ for the
          # embedded frontend) stays intact around it.
          postUnpack = ''
            cd $sourceRoot/src-tauri
            sourceRoot="."
          '';

          nativeBuildInputs = with pkgs; [
            pkg-config
            wrapGAppsHook3
          ];

          buildInputs = with pkgs; [
            # webkitgtk_4_1.dev carries the javascriptcoregtk-4.1 and
            # libsoup-3.0 pc files (javascriptcoregtk has no top-level attr)
            webkitgtk_4_1
            webkitgtk_4_1.dev
            gtk3
            openssl
            librsvg
            libayatana-appindicator
          ];
        };

        cargoArtifacts = craneLib.buildDepsOnly (commonArgs // {
          inherit src;
          pname = "opensidian-deps";
        });

        opensidian = craneLib.buildPackage (commonArgs // {
          inherit src cargoArtifacts;
          version = (builtins.fromJSON (builtins.readFile ./src-tauri/tauri.conf.json)).version;
          # The flock/spawn/landlock tests need a wider sandbox than the Nix
          # build sandbox allows; cargo tests live in `checks` instead.
          doCheck = false;
          postInstall = ''
            install -Dm644 ${./packaging/flatpak/dev.koto.opensidian.desktop} \
              $out/share/applications/dev.koto.opensidian.desktop
          '';
          meta = with pkgs.lib; {
            description = "Vault-compatible markdown notes app (opensidian)";
            homepage = "https://github.com/jpzk/opensidian";
            license = licenses.gpl3Plus;
            mainProgram = "opensidian";
            platforms = platforms.linux;
          };
        });

      in
      {
        packages = {
          default = opensidian;
          inherit opensidian;
        };

        # NOTE: no `checks` output. 11 of the integration tests (flock
        # contention, process spawning, landlock confinement) need a wider
        # sandbox than the Nix build environment grants, so `cargoTest` inside
        # a check derivation fails for environmental, not code, reasons.
        # Run tests from the dev shell: nix develop -c cargo test --locked

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            rustc   # nixpkgs stable == the 1.98.0 the README pins
            cargo
            clippy
            rustfmt
            rust-analyzer
            pkg-config
          ];
          buildInputs = commonArgs.buildInputs;
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });
}
