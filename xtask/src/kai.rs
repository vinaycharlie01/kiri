//! `cargo xtask kai-app` / `kai-dmg`: Kai.app, the one-app install.
//!
//! Kai.app is Kiri.app with Kai as the app you open:
//!
//!   Kai.app/Contents/MacOS/kai        the Kai desktop (CFBundleExecutable)
//!   Kai.app/Contents/MacOS/kiri       Kiri's engine, unchanged and signed with
//!                                     the virtualization entitlement
//!   Kai.app/Contents/Resources/guest  the Linux guest: kernel, initramfs, disk template
//!   Kai.app/Contents/Resources/bin    the docker CLI, Compose and buildx
//!
//! Kai finds `kiri` beside itself and starts `kiri daemon` when no engine is
//! running, so nothing else needs installing. The bundle identifier is
//! Kai's own (`dev.kai.desktop`), which leaves the bundled `kiri` as plain
//! Kiri (crates/kiri/src/flavor.rs): its state is `~/.kiri`, as Kiri.app's.
//!
//! Needs everything `cargo xtask app` needs (guest/out built: `guest`,
//! `kernel`, `disk`) and runs on a Mac: codesign, ditto and hdiutil.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{Arch, Profile, app, run, sign, signing_identity};

/// Kai's bundle identifier; also what keeps the bundled `kiri` plain Kiri.
pub const BUNDLE_ID: &str = "dev.kai.desktop";

fn kai_app_dir(root: &Path, arch: Arch) -> PathBuf {
    root.join(match arch {
        Arch::Arm64 => "target/Kai.app",
        Arch::X86_64 => "target/intel/Kai.app",
    })
}

/// Kiri.app's Info.plist made Kai's: name, identifier and executable. The
/// icon line goes: Kiri's icon is Kiri's brand, and Kai has none yet, so
/// macOS shows its generic app icon.
pub fn kai_plist(kiri_plist: &str) -> io::Result<String> {
    let id = format!("<key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string>");
    let swaps = [
        ("<key>CFBundleName</key><string>Kiri</string>", "<key>CFBundleName</key><string>Kai</string>"),
        ("<key>CFBundleDisplayName</key><string>Kiri</string>", "<key>CFBundleDisplayName</key><string>Kai</string>"),
        ("<key>CFBundleIdentifier</key><string>dev.kiri.app</string>", id.as_str()),
        ("<key>CFBundleExecutable</key><string>kiri</string>", "<key>CFBundleExecutable</key><string>kai</string>"),
        ("  <key>CFBundleIconFile</key><string>Kiri</string>\n", ""),
    ];
    let mut plist = kiri_plist.to_owned();
    for (from, to) in swaps {
        if !plist.contains(from) {
            return Err(io::Error::other(format!("Kiri.app's Info.plist changed: no `{}`; update xtask/src/kai.rs", from.trim())));
        }
        plist = plist.replacen(from, to, 1);
    }
    // The permission prompts name the app the person opened.
    Ok(plist.replace("<string>Kiri lets containers", "<string>Kai lets containers").replace("<string>Kiri shares", "<string>Kai shares").replace("<string>Kiri saves", "<string>Kai saves"))
}

/// Builds Kiri.app, then Kai.app from it. Returns Kai.app's path.
pub fn kai_app(root: &Path, arch: Arch) -> io::Result<PathBuf> {
    let kiri_app = app(root, arch, false, Profile::Release)?;

    // The desktop is its own workspace (desktop/Cargo.toml).
    let desktop = root.join("desktop");
    let mut build = Command::new(env!("CARGO"));
    build.current_dir(&desktop).args(["build", "--release", "--locked", "--bin", "kai"]);
    if !arch.native() {
        build.args(["--target", arch.host_target()]);
    }
    run(&mut build)?;
    let kai_bin = if arch.native() { desktop.join("target/release/kai") } else { desktop.join("target").join(arch.host_target()).join("release/kai") };

    let app = kai_app_dir(root, arch);
    let _ = fs::remove_dir_all(&app);
    if let Some(dir) = app.parent() {
        fs::create_dir_all(dir)?;
    }
    // ditto keeps the signatures already on kiri and the docker tools.
    run(Command::new("ditto").arg(&kiri_app).arg(&app))?;
    let contents = app.join("Contents");
    fs::copy(&kai_bin, contents.join("MacOS/kai"))?;
    let _ = fs::remove_file(contents.join("Resources/Kiri.icns"));
    let plist = fs::read_to_string(contents.join("Info.plist"))?;
    fs::write(contents.join("Info.plist"), kai_plist(&plist)?)?;

    // Inside out: Kai's binary, then the bundle. kiri and the docker tools
    // keep the signatures `app` gave them (kiri with its entitlement).
    let identity = signing_identity();
    run(sign(&identity, None).arg(contents.join("MacOS/kai")))?;
    run(sign(&identity, None).arg(&app))?;
    run(Command::new("codesign").args(["--verify", "--deep", "--strict"]).arg(&app))?;
    println!("{} (signed: {})", app.display(), if identity == "-" { "ad hoc" } else { identity.as_str() });
    Ok(app)
}

/// target/Kai-<version>-arm64.dmg: Kai.app and an Applications shortcut,
/// to drag one onto the other.
pub fn kai_dmg(root: &Path, app: &Path, arch: Arch) -> io::Result<PathBuf> {
    let version = env!("CARGO_PKG_VERSION");
    let stage = root.join("target/kai-dmg");
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage)?;
    run(Command::new("ditto").arg(app).arg(stage.join("Kai.app")))?;
    #[cfg(unix)]
    std::os::unix::fs::symlink("/Applications", stage.join("Applications"))?;
    #[cfg(not(unix))]
    return Err(io::Error::other("a DMG is made on a Mac"));
    let dmg = root.join(format!("target/Kai-{version}{}.dmg", arch.suffix()));
    let _ = fs::remove_file(&dmg);
    run(Command::new("hdiutil").args(["create", "-volname", "Kai", "-fs", "HFS+", "-format", "UDZO", "-srcfolder"]).arg(&stage).arg(&dmg))?;
    fs::remove_dir_all(&stage)?;
    run(sign(&signing_identity(), None).arg(&dmg))?;
    println!("{}", dmg.display());
    Ok(dmg)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KIRI: &str = "<dict>\n  <key>CFBundleName</key><string>Kiri</string>\n  <key>CFBundleDisplayName</key><string>Kiri</string>\n  <key>CFBundleIdentifier</key><string>dev.kiri.app</string>\n  <key>CFBundleExecutable</key><string>kiri</string>\n  <key>CFBundleIconFile</key><string>Kiri</string>\n  <key>NSLocalNetworkUsageDescription</key><string>Kiri lets containers reach devices.</string>\n</dict>\n";

    #[test]
    fn kiris_plist_becomes_kais() {
        let p = kai_plist(KIRI).unwrap();
        assert!(p.contains("<key>CFBundleExecutable</key><string>kai</string>"));
        assert!(p.contains(&format!("<string>{BUNDLE_ID}</string>")));
        assert!(p.contains("<key>CFBundleDisplayName</key><string>Kai</string>"));
        assert!(!p.contains("CFBundleIconFile"));
        assert!(p.contains("Kai lets containers"));
        assert!(!p.contains("dev.kiri.app"), "the bundled kiri must not become a Kiri Dev flavor");
    }

    #[test]
    fn a_changed_kiri_plist_fails_loudly() {
        assert!(kai_plist("<dict></dict>").is_err());
    }

    #[test]
    fn the_plist_written_by_app_is_one_kai_plist_understands() {
        // The real template in main.rs: a change there must fail this test,
        // not a release build on a Mac.
        let main = include_str!("main.rs");
        let start = main.find("<key>CFBundleName</key>").unwrap();
        let end = main[start..].find("</dict>").unwrap() + start;
        let template = main[start..end].replace("{plain}", "1.0");
        kai_plist(&template).unwrap();
    }
}
