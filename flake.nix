{
  description = "Blueboard dev environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    { self, nixpkgs, rust-overlay, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          rustToolchain = pkgs.rust-bin.stable.latest.default.override {
            extensions = [ "rust-src" "rust-analyzer" "clippy" "rustfmt" ];
          };
        in
        {
          default = pkgs.mkShell {
            packages = [
              rustToolchain
              pkgs.pkg-config
              pkgs.lefthook
              # wgpu runtime deps on Linux (Vulkan loader + X11/Wayland libs winit needs)
              pkgs.vulkan-loader
              pkgs.vulkan-tools
              pkgs.libxkbcommon
              pkgs.wayland
              pkgs.libx11
              pkgs.libxcursor
              pkgs.libxi
              pkgs.libxrandr
              (pkgs.texlive.combine {
                inherit (pkgs.texlive)
                  scheme-small
                  algorithms
                  algorithmicx
                  standalone
                  preview
                  amsmath
                  amsfonts
                  ;
              })
              # `pdftoppm`, to rasterize a compiled LaTeX snippet's PDF into a PNG the text
              # tool can upload as a GPU texture — see blueboard-app/src/latex.rs.
              pkgs.poppler-utils
            ];

            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
              pkgs.vulkan-loader
              pkgs.libxkbcommon
              pkgs.wayland
              pkgs.libx11
              pkgs.libxcursor
              pkgs.libxi
              pkgs.libxrandr
            ];
          };
        }
      );
    };
}
