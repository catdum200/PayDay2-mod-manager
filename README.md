# PD2 Mod Manager

A native Windows mod manager for PAYDAY 2, written in Rust. It finds your Epic Games or Steam install by itself, installs mods from **ModWorkshop** and **Nexus Mods** in one click, puts each one in the folder it belongs in, and lets you share your whole setup with a single code.

## Download

Get **[PD2-Mod-Manager-Setup.exe](https://github.com/catdum200/PayDay2-mod-manager/releases/latest/download/PD2-Mod-Manager-Setup.exe)** from the [latest release](https://github.com/catdum200/PayDay2-mod-manager/releases/latest) and run it.
- It installs for your user only, so no admin prompt. You get a Start menu shortcut, an optional desktop shortcut and a normal uninstaller.
- If Windows shows **"Windows protected your PC"**, click **More info → Run anyway**. This appears because the app isn't code-signed.
- Prefer no installer? Download **PD2-Mod-Manager-Portable.zip**, unzip it anywhere and run `pd2-mod-manager.exe`.

Settings and your mod list live in `%APPDATA%\PD2ModManager`. If PAYDAY 2 is under `Program Files` and installs fail with "access denied", run the manager as administrator once.

## Features

- **Finds the game** from Epic Games launcher manifests, then Steam libraries, or lets you pick the folder.
- **Browse ModWorkshop and Nexus** with search, sorting, categories and ModWorkshop tag filters.
- **One-click installs**, or tick several cards and press **Install N mods**. Drag `.zip`, `.7z` or `.rar` archives onto the window to install them.
- **Works out where each mod goes:**
  - SuperBLT mods → `mods/`
  - Asset overrides → `assets/mod_overrides/`
  - BeardLib custom heists → `Maps/`
  - SuperBLT itself → the game root
  - Battle Announcers voice packs → wrapped so Battle Announcers actually loads them
- **Installs requirements automatically.** If a mod needs BeardLib or another mod, it's queued too.
- **Fixes misplaced mods** on startup, such as SuperBLT mods dropped into `mod_overrides`, which crash the game.
- **Library:** enable/disable toggles, uninstall, your own tags plus site tags, filters, and update checks.
- **Launch PAYDAY 2** through Epic or Steam from the top bar.
- **NSFW filter is on by default.** It uses Nexus adult flags plus a keyword check for ModWorkshop, which has no NSFW flag.

## Sharing modpacks

Turn your mod list into a code that friends can paste to install the same mods.

**To share:** go to **Installed → ⬆ Share modpack**, give the pack a name, then **Copy code** or **Save as file…** (`.pd2pack`). Codes are short (about 3 characters per mod, so a 50-mod pack is around 150 characters) and look like `PD2-DFN0ZWFsdGg…`. Paste them into Discord, Steam chat, a forum post or anywhere else.

**To import:** go to **Installed → ⬇ Import modpack** and paste the code (surrounding chat text is fine), or open or drag in a `.pd2pack` file. You'll see which mods you already have, then press **Install** to get the rest.

**To send the files instead:** in the same window, press **📦 Export as zip…**. This packs every mod's files into one `.zip`, including mods you installed from a file, which codes can't include. Your friend drags the zip onto the manager (or uses **Install from file…**), checks the list, and presses **Install**. Mods they already have are skipped unless they tick **Replace**. Zips can be hundreds of MB, so use Google Drive, Dropbox or similar if your chat app limits file sizes.

Good to know:
- Codes list mods by their ModWorkshop or Nexus id, not the files themselves. Everyone downloads the current version from the site, and the import window looks up each mod's name online.
- Mods you installed from a local archive can't be included. The share window lists them so you can send them separately.
- Disabled mods are left out unless you tick **Include disabled mods**.
- NSFW mods in a pack are skipped while your content filter is on.

## Nexus downloads

Browsing doesn't need an account. To download, add your Nexus API key in **Settings**.
- **Premium accounts:** installs happen directly.
- **Free accounts:** Nexus requires the website's download button. Click **Register** in Settings so `nxm://` links open in this app, then press **Mod Manager Download** on Nexus.

## Notes

- Disabled mods are moved to `PAYDAY 2\PD2MM_disabled\` and moved back when you enable them.
- The manager runs as a single instance; `nxm://` links and second launches are handed to the open window.
- ModWorkshop authors can opt out of mod managers. Those mods open in your browser instead.

## Building from source

Requires a recent stable [Rust](https://rustup.rs/) toolchain and the MSVC build tools (for the RAR extractor).

```
cargo build --release
target\release\pd2-mod-manager.exe
```

- Tests: `cargo test`
- Live API tests (hit ModWorkshop and Nexus): `cargo test -- --ignored`

## Releasing

1. Bump `version` in `Cargo.toml` (it's shown in Settings → About) and commit.
2. Push a matching tag, e.g. `git tag v1.0.1 && git push origin v1.0.1`.

`.github/workflows/release.yml` then tests, builds the exe, builds the installer from `installer/setup.iss` with Inno Setup, and publishes a GitHub release with `PD2-Mod-Manager-Setup.exe` and `PD2-Mod-Manager-Portable.zip`. Running the workflow by hand builds the same files as a downloadable artifact without publishing anything.

## License

[MIT](LICENSE)
