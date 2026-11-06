{
  description = "PdfCraft — PDF reader, organiser and security toolkit, a free alternative to Acrobat, in pure Rust";

  inputs = {
    # PdfCraft needs edition 2024 and rustc ≥ 1.95, so the flake tracks unstable. Consumers on a
    # release channel can retarget it: `inputs.pdfcraft.inputs.nixpkgs.follows = "nixpkgs";`
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    # nixpkgs' rustc is linked to an older LLVM than the one this rustc release targets, which breaks
    # the AVX512-VNNI kernels of rten-gemm (`intrinsic signature mismatch for llvm.x86.avx512.vpdpbusd`).
    # The official toolchain (bundled LLVM, what CI builds with) does not have the problem.
    rust-overlay = {
      url = "git+https://github.com/oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # Optional build input of the repository: the fonts that the build embeds when `CRAFT_FONTS_DIR`
    # is set. Official releases always build with them; without them everything works but some
    # scripts have no glyphs. Pinned to the revision the release workflow uses — bump deliberately
    # (`nix flake update craft-fonts`).
    craft-fonts = {
      url = "github:storytold/craft-fonts/3d9ab3a132ed0eff457e424226a435a901da30d3";
      flake = false;
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      craft-fonts,
    }:
    let
      inherit (nixpkgs) lib;

      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      # The AppStream metadata carries a release date. The flake source's own commit date is
      # reproducible, unlike "now"; a tarball or dirty tree without one falls back to the epoch.
      buildDate =
        let
          d = self.lastModifiedDate or "";
        in
        if builtins.stringLength d >= 8 then
          "${builtins.substring 0 4 d}-${builtins.substring 4 2 d}-${builtins.substring 6 2 d}"
        else
          "1970-01-01";

      packageArgs = {
        inherit craft-fonts buildDate;
      };

      # The latest stable release of the official Rust toolchain, as a rustPlatform for `pkgs`.
      rustPlatformFor =
        pkgs:
        let
          toolchain = pkgs.rust-bin.stable.latest.minimal;
        in
        pkgs.makeRustPlatform {
          cargo = toolchain;
          rustc = toolchain;
        };

      mkPackage =
        pkgs: pkgs.callPackage ./nix/package.nix (packageArgs // { rustPlatform = rustPlatformFor pkgs; });

      forAllSystems =
        f:
        lib.genAttrs systems (
          system:
          f (import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          }) system
        );
    in
    {
      packages = forAllSystems (
        pkgs: _:
        let
          pdfcraft = mkPackage pkgs;
        in
        {
          inherit pdfcraft;
          default = pdfcraft;
        }
      );

      # `nixpkgs.overlays = [ inputs.pdfcraft.overlays.default ]` → `pkgs.pdfcraft` everywhere.
      overlays.default = final: _prev: {
        pdfcraft = mkPackage (final.extend rust-overlay.overlays.default);
      };

      apps = forAllSystems (
        _: system: {
          default = {
            type = "app";
            program = lib.getExe self.packages.${system}.pdfcraft;
          };
        }
      );

      devShells = forAllSystems (
        pkgs: system:
        let
          pdfcraft = self.packages.${system}.pdfcraft;
        in
        {
          default = pkgs.mkShell (
            {
              # The package's own native inputs, so `cargo build` in the shell needs no extra setup.
              inputsFrom = [ pdfcraft ];
              packages = with pkgs; [
                cargo
                rustc
                clippy
                rustfmt
                rust-analyzer
              ];
              # `cargo run` dlopens the Vulkan loader, libwayland and libxkbcommon like the packaged binary.
              LD_LIBRARY_PATH = lib.makeLibraryPath pdfcraft.passthru.runtimeLibs;
              RUST_BACKTRACE = "1";
            }
            // lib.optionalAttrs (craft-fonts != null) {
              # Embedded fonts in `cargo run` too; build.rs reads this as CRAFT_FONTS_DIR.
              CRAFT_FONTS_DIR = toString craft-fonts;
            }
          );
        }
      );

      checks = forAllSystems (
        pkgs: system: {
          pdfcraft = self.packages.${system}.pdfcraft;

          # `nix fmt` formats the flake; this keeps it formatted.
          formatting = pkgs.runCommand "pdfcraft-nix-formatting" { nativeBuildInputs = [ pkgs.nixfmt ]; } ''
            nixfmt --check ${./flake.nix} ${./nix}
            touch $out
          '';
        }
      );

      formatter = forAllSystems (pkgs: _: pkgs.nixfmt);
    };
}
