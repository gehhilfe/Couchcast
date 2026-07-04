//! Developer tasks for Couchcast, invoked as `cargo xtask <command>`.
//!
//! Kept dependency-light (only `anyhow`) so it builds fast and never blocks the
//! main app build. Commands mostly orchestrate external tools (cargo, flatpak,
//! python) that are documented in `docs/PACKAGING.md`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

const MANIFEST: &str = "flatpak/io.github.gehhilfe.Couchcast.yml";
const CARGO_SOURCES: &str = "flatpak/cargo-sources.json";

fn main() -> Result<()> {
    let command = std::env::args().nth(1);
    match command.as_deref() {
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
        "cargo xtask <command>\n\n\
         Commands:\n\
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
