//! Developer tasks for Couchcast, invoked as `cargo xtask <command>`.
//!
//! Kept dependency-light (only `anyhow`) so it builds fast and never blocks the
//! main app build. Commands mostly orchestrate external tools (cargo, flatpak,
//! python) that are documented in `docs/PACKAGING.md`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

const APP_ID: &str = "io.github.gehhilfe.Couchcast";
const MANIFEST: &str = "flatpak/io.github.gehhilfe.Couchcast.yml";
const CARGO_SOURCES: &str = "flatpak/cargo-sources.json";

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    // Remaining args are forwarded verbatim to the command's cargo build (e.g.
    // `cargo xtask install --features debug-input-hud`).
    let rest: Vec<String> = args.collect();
    match command.as_deref() {
        Some("install") => install(&rest),
        Some("cargo-sources") => cargo_sources(),
        Some("flatpak-build") => flatpak_build(),
        Some("flatpak-lint") => flatpak_lint(),
        Some("ci") => ci(),
        Some("help") | None => {
            print_help();
            Ok(())
        }
        Some(other) => {
            eprintln!("unknown command: {other}\n");
            print_help();
            bail!("unknown command");
        }
    }
}

fn print_help() {
    println!(
        "cargo xtask <command> [cargo build args]\n\n\
         Commands:\n\
         \x20 install         Build release + install binary/desktop/icon into ~/.local (ready to add to Steam)\n\
         \x20                 Extra args pass through to the build, e.g. install --features debug-input-hud\n\
         \x20 cargo-sources   Regenerate {CARGO_SOURCES} from Cargo.lock (needs flatpak-cargo-generator.py)\n\
         \x20 flatpak-build   Build and install the Flatpak locally via org.flatpak.Builder\n\
         \x20 flatpak-lint    Run flatpak-builder-lint and appstreamcli validate\n\
         \x20 ci              Run the same checks as CI (fmt, clippy, test)\n\
         \x20 help            Show this help\n"
    );
}

/// Workspace root (the parent of this crate's directory).
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask is nested in the workspace")
        .to_path_buf()
}

/// Regenerate the offline crate source list Flathub's network-less builders need.
///
/// Set `COUCHCAST_CARGO_GENERATOR` to the path of `flatpak-cargo-generator.py`
/// (from <https://github.com/flatpak/flatpak-builder-tools>); otherwise the tool
/// is looked up on `PATH`.
fn cargo_sources() -> Result<()> {
    let root = workspace_root();
    let generator = std::env::var("COUCHCAST_CARGO_GENERATOR")
        .unwrap_or_else(|_| "flatpak-cargo-generator.py".to_string());

    run(
        "python3",
        &[generator.as_str(), "Cargo.lock", "-o", CARGO_SOURCES],
        &root,
    )
    .context(
        "failed to run flatpak-cargo-generator.py — install flatpak-builder-tools \
         and/or set COUCHCAST_CARGO_GENERATOR to its path",
    )?;
    println!("wrote {CARGO_SOURCES}");
    Ok(())
}

fn flatpak_build() -> Result<()> {
    let root = workspace_root();
    run(
        "flatpak",
        &[
            "run",
            "org.flatpak.Builder",
            "--user",
            "--install",
            "--force-clean",
            "build-dir",
            MANIFEST,
        ],
        &root,
    )
}

fn flatpak_lint() -> Result<()> {
    let root = workspace_root();
    run(
        "flatpak",
        &[
            "run",
            "--command=flatpak-builder-lint",
            "org.flatpak.Builder",
            "manifest",
            MANIFEST,
        ],
        &root,
    )?;
    run(
        "appstreamcli",
        &[
            "validate",
            "--no-net",
            "data/io.github.gehhilfe.Couchcast.metainfo.xml",
        ],
        &root,
    )
}

fn ci() -> Result<()> {
    let root = workspace_root();
    run("cargo", &["fmt", "--all", "--check"], &root)?;
    run(
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
        &root,
    )?;
    run("cargo", &["test", "--workspace"], &root)
}

/// Build the release binary and install it (plus a desktop entry and icon) into
/// `~/.local`, so it can be launched from a menu or added to Steam as a
/// non-Steam game. The desktop entry's `Exec` is rewritten to the absolute
/// installed path so it launches regardless of `PATH`.
///
/// `cargo_args` are appended to the `cargo build` invocation, so callers can opt
/// into features (e.g. `--features debug-input-hud`) at install time.
fn install(cargo_args: &[String]) -> Result<()> {
    let root = workspace_root();
    let mut build_args = vec!["build", "--release", "-p", "couchcast"];
    build_args.extend(cargo_args.iter().map(String::as_str));
    run("cargo", &build_args, &root)?;

    let home = PathBuf::from(std::env::var("HOME").context("HOME is not set")?);
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));

    let bin_dir = home.join(".local/bin");
    let bin_path = bin_dir.join("couchcast");
    let apps_dir = data.join("applications");
    let icon_dir = data.join("icons/hicolor/scalable/apps");
    for dir in [&bin_dir, &apps_dir, &icon_dir] {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }

    // Binary
    let built = root.join("target/release/couchcast");
    std::fs::copy(&built, &bin_path)
        .with_context(|| format!("copying {} -> {}", built.display(), bin_path.display()))?;
    set_executable(&bin_path)?;

    // Desktop entry with an absolute Exec path
    let template = std::fs::read_to_string(root.join(format!("data/{APP_ID}.desktop")))
        .context("reading desktop template")?;
    let desktop = template.replace("Exec=couchcast", &format!("Exec={}", bin_path.display()));
    let desktop_path = apps_dir.join(format!("{APP_ID}.desktop"));
    std::fs::write(&desktop_path, desktop)
        .with_context(|| format!("writing {}", desktop_path.display()))?;

    // Icon
    let icon_dst = icon_dir.join(format!("{APP_ID}.svg"));
    std::fs::copy(root.join(format!("data/icons/{APP_ID}.svg")), &icon_dst)
        .with_context(|| format!("installing icon -> {}", icon_dst.display()))?;

    let _ = Command::new("update-desktop-database")
        .arg(&apps_dir)
        .status();

    println!("\nInstalled:");
    println!("  binary   {}", bin_path.display());
    println!("  desktop  {}", desktop_path.display());
    println!("  icon     {}", icon_dst.display());
    println!(
        "\nAdd to Steam: Steam → Games → Add a Non-Steam Game… → tick \"Couchcast\" → Add Selected."
    );
    println!("Then open the shortcut's Controller settings and choose a Gamepad layout.");
    if !path_contains(&bin_dir) {
        println!(
            "\n(Note: {} is not on your PATH. The Steam shortcut uses the absolute path so this is fine;",
            bin_dir.display()
        );
        println!(" add it to PATH if you also want to run `couchcast` from a terminal.)");
    }
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn path_contains(dir: &Path) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d == dir))
        .unwrap_or(false)
}

/// Run `program args...` in `cwd`, inheriting stdio, and fail if it exits non-zero.
fn run(program: &str, args: &[&str], cwd: &Path) -> Result<()> {
    println!("$ {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .status()
        .with_context(|| format!("failed to spawn `{program}`"))?;
    if !status.success() {
        bail!("`{program}` exited with {status}");
    }
    Ok(())
}
