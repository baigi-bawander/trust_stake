//! Fails the build if `target/deploy/{truststake,cpi_wrapper}.so` is
//! missing or older than the source that produces it.
//!
//! `tests/common/mod.rs` embeds both files with `include_bytes!`, which only
//! fails if a file is absent -- it has no way to know the bytes it reads are
//! stale. `cargo test` compiles this crate with the native host toolchain
//! and never invokes `anchor build` (a different toolchain, a different
//! target), so editing this program's source and re-running the suite
//! without an intervening `anchor build` would otherwise silently test
//! whatever program was last built into `target/deploy/`. That is not
//! hypothetical: checking out a branch whose program is entirely different
//! code, built into this same `target/`, leaves exactly this trap, and nothing
//! before this script existed would notice.
//!
//! This only runs for native builds. `anchor build` itself cross-compiles
//! this same crate for Solana's SBF target, which also runs this build
//! script -- build scripts always run on the host, even when the crate they
//! belong to is being cross-compiled for somewhere else -- at the exact
//! moment the `.so` this check looks for does not exist yet. Checking on
//! that pass would make `anchor build` unable to ever produce a first
//! artifact. Detected by `TARGET` and `CARGO_CFG_TARGET_OS`, which Cargo
//! always hands a build script and which name Solana's target only during
//! that cross-compiling pass, never during `cargo test`/`check`/`clippy`.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// The bytes `tests/common/mod.rs` embeds via `include_bytes!`, and
/// everything that determines them: the program's own source directory,
/// plus its manifest, since a dependency bump changes the built program
/// without touching a single file under `source_dir`.
struct EmbeddedProgram {
    so_path: PathBuf,
    /// Shown in the failure message, e.g. `programs/truststake/src/`.
    source_label: &'static str,
    source_dir: PathBuf,
    manifest: PathBuf,
}

fn main() {
    let target = env::var("TARGET").unwrap_or_default();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target.contains("solana") || target_os == "solana" {
        return;
    }

    let manifest_dir = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set by Cargo"),
    );
    // `tests/common/mod.rs` reaches `target/deploy/*.so` with
    // `../../../../` from its own directory, four levels up from
    // `programs/truststake/tests/common/`. From this package's root
    // (`programs/truststake`, two of those four levels already spent) the
    // same place is two levels up.
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect(
            "programs/truststake has two parent directories: programs/, then the workspace root",
        )
        .to_path_buf();
    let deploy_dir = workspace_root.join("target/deploy");
    let workspace_lockfile = workspace_root.join("Cargo.lock");

    let programs = [
        EmbeddedProgram {
            so_path: deploy_dir.join("truststake.so"),
            source_label: "programs/truststake/src/",
            source_dir: manifest_dir.join("src"),
            manifest: manifest_dir.join("Cargo.toml"),
        },
        EmbeddedProgram {
            so_path: deploy_dir.join("cpi_wrapper.so"),
            source_label: "programs/cpi_wrapper/src/",
            source_dir: workspace_root.join("programs/cpi_wrapper/src"),
            manifest: workspace_root.join("programs/cpi_wrapper/Cargo.toml"),
        },
    ];

    for program in &programs {
        check(program, &workspace_lockfile);
    }
}

fn check(program: &EmbeddedProgram, workspace_lockfile: &Path) {
    let so_modified = match fs::metadata(&program.so_path) {
        Ok(metadata) => modified(&program.so_path, &metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => fail_missing(program),
        Err(error) => panic!(
            "failed to read metadata of {}: {error}",
            program.so_path.display()
        ),
    };

    let (newest_path, newest_modified) = [
        newest_in(&program.source_dir),
        (program.manifest.clone(), mtime(&program.manifest)),
        (workspace_lockfile.to_path_buf(), mtime(workspace_lockfile)),
    ]
    .into_iter()
    .max_by_key(|(_, modified)| *modified)
    .expect("the list of candidates above is never empty");

    if newest_modified > so_modified {
        fail_stale(program, &newest_path);
    }
}

/// The most recently modified file anywhere under `dir`, recursively.
fn newest_in(dir: &Path) -> (PathBuf, SystemTime) {
    let mut newest: Option<(PathBuf, SystemTime)> = None;
    visit(dir, &mut |path| {
        let modified = mtime(path);
        if newest
            .as_ref()
            .is_none_or(|(_, current)| modified > *current)
        {
            newest = Some((path.to_path_buf(), modified));
        }
    });
    newest.unwrap_or_else(|| panic!("found no source files under {}", dir.display()))
}

fn visit(dir: &Path, visitor: &mut impl FnMut(&Path)) {
    let entries = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("failed to read directory {}: {error}", dir.display()));
    for entry in entries {
        let entry = entry.expect("failed to read directory entry");
        let path = entry.path();
        let is_dir = entry
            .file_type()
            .unwrap_or_else(|error| {
                panic!("failed to read file type of {}: {error}", path.display())
            })
            .is_dir();
        if is_dir {
            visit(&path, visitor);
        } else {
            visitor(&path);
        }
    }
}

fn mtime(path: &Path) -> SystemTime {
    let metadata = fs::metadata(path)
        .unwrap_or_else(|error| panic!("failed to read metadata of {}: {error}", path.display()));
    modified(path, &metadata)
}

fn modified(path: &Path, metadata: &fs::Metadata) -> SystemTime {
    metadata
        .modified()
        .unwrap_or_else(|error| panic!("failed to read mtime of {}: {error}", path.display()))
}

fn fail_missing(program: &EmbeddedProgram) -> ! {
    panic!(
        "\n\n\
         {so} does not exist.\n\
         tests/common/mod.rs embeds it with `include_bytes!`, so nothing in this crate can \
         compile without it.\n\
         \n\
         Run `anchor build` from the repo root, then try again.\n\n",
        so = program.so_path.display(),
    )
}

fn fail_stale(program: &EmbeddedProgram, newest_path: &Path) -> ! {
    panic!(
        "\n\n\
         {so} is older than {newest_path}.\n\
         `cargo test` compiles this crate with the native host toolchain and never invokes \
         `anchor build`, so an edit under {label} is not reflected in the compiled program: \
         tests/common/mod.rs would embed the stale `.so` with `include_bytes!` and silently \
         test the OLD program.\n\
         \n\
         Run `anchor build` from the repo root, then try again.\n\n",
        so = program.so_path.display(),
        newest_path = newest_path.display(),
        label = program.source_label,
    )
}
