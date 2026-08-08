# Nexo Client

A native Minecraft launcher, written from scratch in Rust.

No webview, no bundled browser engine, no JVM — the UI is
[iced](https://iced.rs), which renders on the GPU through `wgpu`, and the whole
app ships as a single compiled binary.

This is the install/launch half of Nexo. The other half is
[Nexo Mod](https://github.com/Lokifisch/nexo-mod), a Fabric mod that this
launcher installs into instances.

> **Status:** early. Instances and Microsoft sign-in work end to end.
> Mod browsing is groundwork only — see [Scope](#scope).

## Why

An earlier version of this was a fork of Modrinth's desktop app. That worked,
but it carried GPL-3.0 obligations, a Vue/Tauri webview stack, and a large
vendored surface that had to be continually rebranded and pruned. Writing the
launcher directly turned out to be a smaller problem than maintaining someone
else's.

The design constraints that fell out of that:

- **Native compiled, no webview.** iced renders through `wgpu` (Vulkan/DX12/
  Metal). Startup is instant and idle memory stays low.
- **MIT throughout**, so there's no licensing entanglement to work around.
- **Windows and Linux first**, macOS after.

## Layout

Two crates:

- **`crates/nexo-core`** — everything with logic in it, and no UI dependency at
  all: instance storage, the Microsoft auth chain, Java discovery, the Mojang
  download pipeline, Fabric resolution, and launch-command construction.
  Headless on purpose, so the launch path stays testable without opening a
  window.
- **`crates/nexo-app`** — the iced frontend. Deliberately thin: `update` is a
  pure state transition and every real operation is an async `Task` calling
  into `nexo-core`. That split is what keeps the window painting while a
  ~1,000-file install runs underneath it.

## Scope

Working today:

- **Instances** — create, list, delete. Each is a self-describing directory
  with its own saves, mods, and config; its metadata lives in a
  `nexo-instance.json` inside it, so an instance can be copied or backed up
  without the launcher losing track of it, and one corrupt manifest can't hide
  the rest.
- **Microsoft sign-in** — the OAuth2 device-code flow, then the full
  MSA → Xbox Live → XSTS → Minecraft Services exchange. Multiple accounts, one
  active at a time, with silent token refresh before launch. Xbox's numeric
  error codes are translated into actual sentences rather than surfaced raw.
- **Install and launch** — resolves the version from Mojang's manifest, layers
  Fabric's profile on top, downloads the client jar, libraries, and assets in
  parallel, unpacks natives, and builds the JVM command.

Groundwork, no UI yet:

- **Modrinth** — `nexo-core::modrinth` is written and tested (search, version
  resolution, checksum-verified downloads) but nothing calls it from the
  frontend.

Not started: CurseForge, modpack import, mod updating, an in-app log console,
and auto-installing Nexo Mod into an instance.

## Building

Needs a Rust toolchain (pinned by `rust-toolchain.toml`) and, on Linux, the
usual windowing/graphics dev packages for `winit` + `wgpu`.

```sh
cargo run            # debug build
cargo run --release  # what you'd actually play on
```

## Testing

```sh
cargo test                                # unit tests, offline
cargo test --test live_apis -- --ignored  # contract tests against the real APIs
```

Run the second set after touching anything in `minecraft::meta`,
`minecraft::fabric`, or `modrinth`. Every type in those modules is an
assumption about someone else's JSON, and upstream drift would otherwise show
up as a launch failure in the UI rather than a failing test.

## Notes

- Instances, libraries, assets, and accounts live under the platform data
  directory (`~/.local/share/nexo` on Linux, `%APPDATA%\Nexo\data` on Windows).
  Libraries and assets are shared across instances deliberately — they're
  identical per version, so per-instance copies would waste ~200 MB apiece.
- Account tokens are currently stored with owner-only file permissions. That's
  weaker than the AES-256-GCM at-rest encryption Nexo Mod uses, and matching it
  is worth doing before any wider release.

## License

MIT — see [`LICENSE`](LICENSE).
