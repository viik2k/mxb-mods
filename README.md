# mxb-mods

Installs MX Bikes mods (zip / rar / 7z / pkz / folders) into the right place in the mods folder
without wrecking their layout. Works on Windows and the Steam Deck (Proton prefix, internal or SD card).

    mxb-mods                     open the window: Downloads list, drag-and-drop, installed mods
    mxb-mods install <path>      install an archive, folder or .pkz
    mxb-mods list                list installed mods

Set `MXB_MODS` to override the mods folder.

## Build

Windows:

    cargo build --release

Steam Deck (from any machine with Docker; the binary needs glibc 2.35+):

    docker run --rm -v "$PWD:/src" -w /src -e CARGO_TARGET_DIR=/src/target/linux rust:bookworm cargo build --release

Output: `target/linux/release/mxb-mods`. On the Deck it needs `bsdtar` (shipped with SteamOS).
