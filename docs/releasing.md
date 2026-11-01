# Packaging

Etch publishes no releases yet. The packaging scripts build installers locally:

- Linux: `packaging/linux/package.sh` (then `packaging/linux/flatpak-bundle.sh` for a Flatpak)
- macOS: `packaging/macos/package.sh --arch universal`
- Windows: `packaging/windows/package.ps1`
- Web: `packaging/web/package.sh`

The version lives in `[workspace.package] version` in the root `Cargo.toml`
(`cargo xtask version set <version>`); the app shows it in `--version` and About, with the commit
from `WORDCRAFT_BUILD_SHA`.

Build profiles (root `Cargo.toml`): the Linux and macOS packaging builds with
`cargo build --profile dist` (output in `target/dist`, or `target/<triple>/dist` when
cross-compiling): the release profile with fat LTO and `codegen-units = 1`, and `opt-level = "s"`
for a few large crates whose speed doesn't matter (naga, zbus, zvariant, image-webp, tiff). It
builds more slowly than `--release` and gives smaller, faster binaries. `WORDCRAFT_PROFILE=release`
switches the packaging scripts back to `target/release`. The release profile strips its binaries
(`strip = true`) and keeps `panic = "unwind"` explicitly, as every profile must: `Session::run`
catches an escaped panic and restores the document, and `panic = "abort"` would make that a crash.
