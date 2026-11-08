{
  description = "PdfCraft: package, Home Manager module and dev shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      ...
    }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      };
      rust = pkgs.rust-bin.stable.latest.default.override {
        extensions = [
          "rust-src"
          "rust-analyzer"
        ];
        targets = [ "wasm32-unknown-unknown" ];
      };
      pdfcraftFor =
        p:
        let
          toolchain = p.rust-bin.stable.latest.minimal;
        in
        p.callPackage ./nix/package.nix {
          rustPlatform = p.makeRustPlatform {
            cargo = toolchain;
            rustc = toolchain;
          };
        };
      runtimeLibs = with pkgs; [
        dbus
        libGL
        libx11
        libxcursor
        libxi
        libxkbcommon
        libxrandr
        vulkan-loader
        wayland
      ];
      launcher = pkgs.writeShellApplication {
        name = "x-craft";
        runtimeInputs = [ pkgs.git ];
        text = ''
          root=$(git rev-parse --show-toplevel)
          exec cargo run --release --manifest-path "$root/Cargo.toml" -p pdfcraft -- "$@"
        '';
      };
    in
    {
      packages.${system} = rec {
        pdfcraft = pdfcraftFor pkgs;
        default = pdfcraft;
      };

      overlays.default = nixpkgs.lib.composeExtensions rust-overlay.overlays.default (
        final: _prev: { pdfcraft = pdfcraftFor final; }
      );
      homeModules.default = import ./nix/hm-module.nix self;

      devShells.${system}.default = pkgs.mkShell {
        nativeBuildInputs = with pkgs; [
          cmake
          launcher
          pkg-config
          rust
          rustPlatform.bindgenHook
          cargo-deny
          just
        ];
        buildInputs =
          with pkgs;
          [
            fontconfig
            pipewire
            libdrm
            libgbm
          ]
          ++ runtimeLibs;
        LD_LIBRARY_PATH = "/run/opengl-driver/lib:" + pkgs.lib.makeLibraryPath runtimeLibs;
        shellHook = ''
          rustc --version
        '';
      };
    };
}
