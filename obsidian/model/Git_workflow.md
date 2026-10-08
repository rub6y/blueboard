# Git workflow

The git workflow uses trunk-based development: a single long-lived `main`, short-lived
feature branches.

The git workflow uses conventional commit messages (`feat:`, `fix:`, `refactor:`, etc.).

The git workflow tags each [[planning_PLAN]] milestone (e.g. `v0.1-window-and-camera`).

The git workflow runs `cargo fmt --check` and `cargo clippy -- -D warnings` as a
`lefthook` pre-commit hook, provided by the [[Dev_environment]].
