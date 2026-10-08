# Dev environment

The dev environment is a Nix flake (`flake.nix`) plus `direnv` (`.envrc`).

The dev environment pins the Rust toolchain via `rust-overlay` rather than relying on an
imperatively installed one.

The dev environment includes the wgpu/winit runtime libraries (Vulkan loader,
libxkbcommon, Wayland/X11 libs) needed to run [[Blueboard]] on this NixOS machine.

The dev environment includes a pinned TeX Live subset (via `texlive.combine`) so
`planning/math.tex` can be typeset.

The dev environment includes `lefthook`, which runs the [[Git_workflow]]'s pre-commit
checks.
