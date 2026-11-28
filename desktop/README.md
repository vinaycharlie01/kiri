# Kai — install on a MacBook

Kai is a native desktop app (written in Rust, on GPUI) for managing and, later,
running AI agents on top of [Kiri](https://github.com/libracoder/kiri), a fast
container engine. Kai is a **client of Kiri**: you need Kiri installed and
running first.

This guide starts from a MacBook with nothing installed.

> **Status: early milestone.** Kai currently shows Kiri's engine status and
> your containers, and can start, stop, restart, pause, resume and remove
> them. AI agent features come in later phases. It has been built and tested
> on Linux and type-checked for Apple silicon, but **not yet run on a Mac**:
> if something goes wrong, please report what you see (see the end).

## What you need

- A MacBook with **macOS 14 (Sonoma) or later**, Apple silicon (M1–M4) or Intel
- About 5 GB of free disk space (Rust, the build, and Kiri)
- An internet connection

## Step 1 — Install Apple's command-line tools

Open **Terminal** (⌘ Space, type `Terminal`, press Return) and run:

```sh
xcode-select --install
```

Click **Install** in the window that appears and wait for it to finish. If it
says the tools are already installed, continue.

## Step 2 — Install Homebrew

Skip this if `brew --version` already works.

```sh
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
```

At the end, the installer prints two or three commands under **Next steps**
(they add Homebrew to your PATH). Copy and run them, then check:

```sh
brew --version
```

## Step 3 — Install Kiri and start it

```sh
brew install --cask libracoder/kiri/kiri
```

(Or download the `.dmg` from [runkiri.com/releases](https://runkiri.com/releases/)
and drag Kiri to Applications.)

Then open **Kiri** from Applications (or `open -a Kiri`). The first launch
sets Kiri up; follow its window. Check that the engine answers:

```sh
curl --unix-socket ~/.kiri/run/docker.sock http://d/_ping
```

It should print `OK`. Then run a first container:

```sh
kiri run --rm alpine echo hello
```

## Step 4 — Install Rust

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Press Return to accept the defaults, then load it into this Terminal:

```sh
source "$HOME/.cargo/env"
rustc --version
```

Kai needs Rust **1.95 or newer**. If yours is older: `rustup update stable`.

## Step 5 — Download Kai

```sh
git clone https://github.com/vinaycharlie01/kiri.git
cd kiri
git checkout ccr-9cbb0820-vdtpou
```

(Once the pull request is merged, skip the `git checkout` line.)

## Step 6 — Build and open Kai

```sh
cd desktop
cargo run --release --bin kai
```

The first build downloads and compiles about 500 packages: expect **5–15
minutes**. Later starts take seconds. A window titled **Kai** opens.

Kai follows macOS's light or dark appearance. To choose one yourself:

```sh
cargo run --release --bin kai -- --theme dark     # or: --theme light
```

To keep a copy of the app you can run without rebuilding:

```sh
sudo mkdir -p /usr/local/bin
sudo cp target/release/kai /usr/local/bin/kai    # asks for your Mac password; then just run: kai
```

## Step 7 — Try it

With Kai open, in another Terminal tab (⌘ T):

```sh
kiri run -d --name web -p 8080:80 nginx
```

`web` appears in Kai's **Containers** section within a second. Try **Stop**,
**Start**, **Pause** and **Resume**. **Remove** (on a stopped container) asks
you to confirm before anything is deleted.

Keyboard shortcuts:

| Keys | Action |
|------|--------|
| ⌘ 1 | Overview |
| ⌘ 2 | Containers |
| ⌘ R | Refresh |
| ⌘ ⇧ A | Show or hide stopped containers |

## Check that everything works (optional)

From the `kiri` folder (one level above `desktop`):

```sh
# Unit and architecture tests: no engine needed
cargo test -p kai-domain -p kai-application -p kai-testkit -p kai-adapter-kiri
(cd desktop && cargo test)

# Against your running Kiri, including a container's whole lifecycle
kiri pull alpine:3.21
KAI_TEST_KIRI_SOCKET=$HOME/.kiri/run/docker.sock KAI_TEST_KIRI_IMAGE=alpine:3.21 \
  cargo test -p kai-adapter-kiri --test real_engine
```

The lifecycle test creates a container named `kai-test-life-…`, takes it
through start, pause, resume, restart and stop, then removes it.

## Troubleshooting

| What you see | What to do |
|--------------|------------|
| Kai says **"Not reachable"** | Kiri isn't running. Open Kiri from Applications. Kai reconnects by itself within a few seconds. |
| You use **Kiri Dev** | Point Kai at its socket: `kai --socket ~/.kiri-dev/run/docker.sock` |
| `error: package ... requires rustc 1.95` | `rustup update stable`, then build again. |
| `xcrun: error` or `linker 'cc' not found` | The command-line tools are missing: repeat Step 1. |
| `brew: command not found` after Step 2 | Run the **Next steps** commands Homebrew printed, or open a new Terminal window. |
| The window doesn't open, or looks wrong | Please report it (below). |

Kai is a plain program run from Terminal for now, not a `.app` in
Applications; it shows a generic icon in the Dock. A proper app bundle is on
the roadmap.

## Reporting a problem

Open an issue at <https://github.com/vinaycharlie01/kiri/issues> with:

- your Mac model and macOS version ( → About This Mac)
- the full Terminal output of the command that failed
- a screenshot, if the window opened

## Uninstall

```sh
rm -rf ~/path/to/kiri                 # the folder you cloned in Step 5
sudo rm -f /usr/local/bin/kai         # if you copied it there
brew uninstall --cask kiri            # Kiri itself, if you want it gone too
```
