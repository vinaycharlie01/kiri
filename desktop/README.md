# Kai — install on a MacBook

Kai is a native desktop app (written in Rust, on GPUI) for managing and, later,
running AI agents on containers. **Kai comes with its own container engine**,
Kiri's: you install one app, and Kai starts the engine itself. There is no
Docker Desktop, no separate Kiri and no Rust toolchain to install.

> **Status: early milestone.** Kai shows the engine's status and your
> containers, and can start, stop, restart, pause, resume and remove them.
> AI agent features come in later phases.

## What you need

- A MacBook with **Apple silicon (M1 or later)** and **macOS 14 (Sonoma) or later**
- About 1 GB of free disk space

## Install

1. **Download** `Kai-<version>-arm64.dmg`:
   - from the repository's [Releases](https://github.com/vinaycharlie01/kiri/releases)
     (look for a `kai-v…` release), or
   - from the latest successful
     [Kai macOS app](https://github.com/vinaycharlie01/kiri/actions/workflows/kai-macos.yml)
     run: open the run, scroll to **Artifacts**, download **Kai-macOS-arm64**
     and unzip it.
2. **Open the DMG** and drag **Kai** onto **Applications**.
3. **Open Kai** from Applications. The first time, macOS says it can't verify
   the developer (this build is not notarized by Apple yet):
   - click **Done** (or **Cancel**),
   - open **System Settings → Privacy & Security**, scroll down, and click
     **Open Anyway** next to "Kai was blocked",
   - confirm with **Open Anyway** and your password.

   You only do this once.

Kai opens and starts its engine: the status bar says **Starting Kiri's
engine…**, then **Kiri … · 0 containers**. That's it.

## Try it

Kai's engine also gives you the `kiri` command. Add it to your Terminal once:

```sh
echo 'export PATH="/Applications/Kai.app/Contents/MacOS:$PATH"' >> ~/.zshrc
source ~/.zshrc
```

Then run a container:

```sh
kiri run -d --name web -p 8080:80 nginx
```

`web` appears in Kai's **Containers** section within a second, and
<http://localhost:8080> shows nginx's page. In Kai, try **Stop**, **Start**,
**Pause** and **Resume**; **Remove** (on a stopped container) asks you to
confirm before anything is deleted.

| Keys | Action |
|------|--------|
| ⌘ 1 | Overview |
| ⌘ 2 | Containers |
| ⌘ R | Refresh |
| ⌘ ⇧ A | Show or hide stopped containers |

To choose dark or light yourself instead of following macOS:
`open -a Kai --args --theme dark` (or `light`).

## Good to know

- **The engine keeps running after you quit Kai**, so your containers keep
  running too. Opening Kai again connects to it.
- Kai keeps the engine's data where Kiri does: `~/.kiri` and
  `~/Library/Application Support/Kiri`. If you install Kiri itself later, it
  uses the same engine and containers.
- The first container you start takes a few seconds longer: the engine boots
  its small Linux VM on first use.

## Troubleshooting

| What you see | What to do |
|--------------|------------|
| "Kai is damaged and can't be opened" | The download was quarantined. In Terminal: `xattr -dr com.apple.quarantine /Applications/Kai.app`, then open Kai again. |
| Kai says **Could not start Kiri's engine** | The message names a log file (`~/.kiri/daemon.log`). Click **Start engine** on the Overview to try again; if it fails again, report it with that log. |
| `kiri: command not found` | Do the PATH step under **Try it**, or open a new Terminal window. |
| You have an Intel Mac | This build is for Apple silicon. An Intel build is possible (`--arch x86_64`) but not published yet. |

## Reporting a problem

Open an issue at <https://github.com/vinaycharlie01/kiri/issues> with your
Mac model and macOS version ( → About This Mac), what you did, a screenshot,
and `~/.kiri/daemon.log` if the engine is involved.

## Uninstall

Quit Kai, then:

```sh
kiri rm -f $(kiri ps -aq) 2>/dev/null     # your containers, if you want them gone
pkill -f 'Kai.app/Contents/MacOS/kiri'    # the engine
rm -rf /Applications/Kai.app
rm -rf ~/.kiri "$HOME/Library/Application Support/Kiri"   # all engine data: images, volumes
```

## For developers: building Kai.app yourself

The DMG is built by [`.github/workflows/kai-macos.yml`](../.github/workflows/kai-macos.yml):
a Linux job builds the guest kernel and disk template (they need Linux
tools), and a macOS job builds everything else and packs the app with
`cargo xtask kai-dmg` ([`xtask/src/kai.rs`](../xtask/src/kai.rs)). Push a
`kai-v<version>` tag to publish a release.

To build on your own Mac you need the same inputs as `cargo xtask app`
(see [docs/development.md](../docs/development.md)): Xcode's command-line
tools, Rust, `rustup target add aarch64-unknown-linux-musl`, and the guest
in `guest/out` (`cargo xtask kernel` and `cargo xtask disk` need a container
engine, or take `Image` and `data.img` from the workflow's `kai-guest-arm64`
artifact). Then:

```sh
cargo xtask guest
cargo xtask kai-dmg        # target/Kai.app and target/Kai-<version>-arm64.dmg
```

For working on Kai itself:

```sh
cd desktop
cargo run --bin kai -- --kiri ../target/release/kiri   # --kiri: an engine Kai may start
cargo test
```
