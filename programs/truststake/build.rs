//! Fails the build if `target/deploy/{truststake,cpi_wrapper}.so` is
//! missing or older than the source that produces it.
//!
//! `tests/common/mod.rs` loads both files at run time, which only fails if
//! a file is absent -- it has no way to know the bytes it reads are stale.
//! `cargo test` compiles this crate with the native host toolchain and
//! never invokes `anchor build` (a different toolchain, a different
//! target), so editing this program's source and re-running the suite
//! without an intervening `anchor build` would otherwise silently test
//! whatever program was last built into `target/deploy/`. That is not
//! hypothetical: checking out a branch whose program is entirely different
//! code, built into this same `target/`, leaves exactly this trap, and
//! nothing before this script existed would notice.
//!
//! # Which passes this check runs on, and why it skips two of them
//!
//! `anchor build` compiles this crate twice per program, and processes the
//! workspace's programs strictly one at a time and completely:
//!
//! 1. `truststake`, SBF pass -- `cargo build-sbf`, `TARGET=sbpf-solana-solana`.
//! 2. `truststake`, IDL pass -- the *host* toolchain,
//!    `TARGET=x86_64-unknown-linux-gnu`, `--features idl-build`.
//! 3. `cpi_wrapper`, SBF pass.
//! 4. `cpi_wrapper`, IDL pass.
//!
//! Build scripts always run on the host even when the crate they belong to
//! is cross-compiled, so this script runs on all four. Two of them must not
//! check anything:
//!
//! * The **SBF pass** runs at the exact moment the `.so` it would look for
//!   does not exist yet. Checking there would make `anchor build` unable to
//!   ever produce a first artifact. Detected by `TARGET` /
//!   `CARGO_CFG_TARGET_OS`, which name Solana's target only during that
//!   cross-compiling pass.
//!
//! * The **IDL pass** is a native host build, so the target check above does
//!   not catch it -- and it happens at step 2, before `cpi_wrapper` has been
//!   built at step 3. A check here demands a fresh `cpi_wrapper.so` during
//!   the very command that produces it, so `anchor build` can never succeed
//!   once anything makes that file look stale. Detected by
//!   `CARGO_FEATURE_IDL_BUILD`, which Cargo sets because Anchor passes
//!   `--features idl-build`; verified by dumping this script's environment
//!   on all four passes rather than taken from documentation.
//!
//! # Why dependency inputs are compared by content, not by mtime
//!
//! `Cargo.lock` and the two manifests are real inputs: a dependency bump
//! changes a built program without touching a single file under `src/`.
//! Comparing their *mtimes* against a `.so`, however, produces a state that
//! `anchor build` provably cannot clear, which is worse than not checking
//! them at all:
//!
//! * `Cargo.lock`'s mtime moves for reasons that change nothing -- any
//!   `cargo` invocation that re-resolves, a `git checkout`, an editor save.
//! * Cargo's freshness model is content-based, so none of that makes it
//!   rebuild anything, and `cargo build-sbf` re-copies into
//!   `target/deploy/` only when the compiled artifact actually changed or
//!   the copy is missing. The `.so`'s mtime therefore stays put.
//! * One workspace `Cargo.lock` is shared by both programs, so a resolve
//!   affecting only `truststake`'s graph still outruns `cpi_wrapper.so`,
//!   which cargo correctly declines to rebuild.
//!
//! Once the lockfile's mtime passes a `.so`'s, this script's own advice
//! ("run `anchor build`") is unsatisfiable and the only escape is deleting
//! the artifact -- which is how this check was last "fixed", and why the
//! failure came straight back the next time anything wrote `Cargo.lock`.
//!
//! So those three inputs are compared against copies of themselves taken at
//! the last build, in `target/deploy/.build-guard/`, written by the SBF
//! pass above -- the one moment that means "cargo is building this program
//! from these inputs, right now". `src/` stays on mtimes: an edit there is
//! the case this guard exists for, and cargo's own fingerprints are
//! mtime-based for source files too, so the two agree.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// The bytes `tests/common/mod.rs` reads out of `target/deploy/`, and the
/// source directory that determines them.
struct EmbeddedProgram {
    so_path: PathBuf,
    /// Shown in the failure message, e.g. `programs/truststake/src/`.
    source_label: &'static str,
    source_dir: PathBuf,
}

/// One file whose *contents* feed the built programs: shown as
/// `stamp_name` inside `target/deploy/.build-guard/`.
struct DependencyInput {
    path: PathBuf,
    stamp_name: &'static str,
}

fn main() {
    let manifest_dir = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set by Cargo"),
    );
    // `tests/common/mod.rs` reaches `target/deploy/` with `../../` from
    // this package's root (`programs/truststake`), which is two levels
    // below the workspace root.
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect(
            "programs/truststake has two parent directories: programs/, then the workspace root",
        )
        .to_path_buf();
    let deploy_dir = workspace_root.join("target/deploy");
    let stamp_dir = deploy_dir.join(".build-guard");

    let dependency_inputs = [
        DependencyInput {
            path: workspace_root.join("Cargo.lock"),
            stamp_name: "Cargo.lock",
        },
        DependencyInput {
            path: manifest_dir.join("Cargo.toml"),
            stamp_name: "truststake.Cargo.toml",
        },
        DependencyInput {
            path: workspace_root.join("programs/cpi_wrapper/Cargo.toml"),
            stamp_name: "cpi_wrapper.Cargo.toml",
        },
    ];

    // Cargo re-runs a build script whose directives name a changed path.
    // Naming any path at all replaces the default "rescan the whole
    // package", so everything this script reads has to be listed -- and
    // `Cargo.lock` and the sibling program, which that default never
    // covered, finally are. Without the lockfile listed here, a `cargo
    // update` that leaves this package's own files alone would not re-run
    // the SBF pass, the stamps below would never be refreshed, and the
    // content check would be as unclearable as the mtime one it replaces.
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=src");
    println!(
        "cargo::rerun-if-changed={}",
        workspace_root.join("programs/cpi_wrapper/src").display()
    );
    for input in &dependency_inputs {
        println!("cargo::rerun-if-changed={}", input.path.display());
        // The stamps too, so that wiping `target/deploy/` -- which does not
        // touch a single tracked source file, and so would otherwise leave
        // cargo with no reason to re-run this script -- does not silently
        // retire the content check below until something else happens to
        // change. Cargo treats a path that does not exist as changed, so a
        // missing stamp re-runs this script until the SBF pass writes it
        // back. `write_stamps` only writes on a difference, so a stamp that
        // is already correct never moves its own mtime and can never
        // re-trigger itself.
        println!(
            "cargo::rerun-if-changed={}",
            stamp_dir.join(input.stamp_name).display()
        );
    }

    let target = env::var("TARGET").unwrap_or_default();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target.contains("solana") || target_os == "solana" {
        // The SBF pass: cargo is building a program from these inputs right
        // now, so record them. Best-effort -- a `target/deploy/` that cannot
        // be written is not a reason to fail a build that would otherwise
        // succeed; a missing stamp only costs the content check below.
        write_stamps(&stamp_dir, &dependency_inputs);
        return;
    }
    if env::var_os("CARGO_FEATURE_IDL_BUILD").is_some() {
        // Anchor's IDL pass, which runs before the sibling program exists.
        return;
    }

    let programs = [
        EmbeddedProgram {
            so_path: deploy_dir.join("truststake.so"),
            source_label: "programs/truststake/src/",
            source_dir: manifest_dir.join("src"),
        },
        EmbeddedProgram {
            so_path: deploy_dir.join("cpi_wrapper.so"),
            source_label: "programs/cpi_wrapper/src/",
            source_dir: workspace_root.join("programs/cpi_wrapper/src"),
        },
    ];

    for program in &programs {
        check(program);
    }
    for input in &dependency_inputs {
        check_dependency_input(&stamp_dir, input);
    }
}

fn write_stamps(stamp_dir: &Path, inputs: &[DependencyInput]) {
    if fs::create_dir_all(stamp_dir).is_err() {
        return;
    }
    for input in inputs {
        let Ok(bytes) = fs::read(&input.path) else {
            continue;
        };
        let stamp_path = stamp_dir.join(input.stamp_name);
        // Written only when it would actually differ: an unconditional
        // write would move the stamp's mtime on every build, and this
        // script asks cargo to re-run it whenever a stamp changes.
        if fs::read(&stamp_path).is_ok_and(|stamped| stamped == bytes) {
            continue;
        }
        let _ = fs::write(&stamp_path, bytes);
    }
}

fn check(program: &EmbeddedProgram) {
    let so_modified = match fs::metadata(&program.so_path) {
        Ok(metadata) => modified(&program.so_path, &metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => fail_missing(program),
        Err(error) => panic!(
            "failed to read metadata of {}: {error}",
            program.so_path.display()
        ),
    };

    let (newest_path, newest_modified) = newest_in(&program.source_dir);
    if newest_modified > so_modified {
        fail_stale(program, &newest_path);
    }
}

/// Compares one dependency input against the copy the last SBF build took
/// of it. A missing stamp proves nothing and is not an error: it just means
/// nothing has recorded which inputs the artifacts were built from -- a
/// `target/` restored from a cache, say -- and the `src/` check above still
/// covers the case this guard exists for.
fn check_dependency_input(stamp_dir: &Path, input: &DependencyInput) {
    let stamp_path = stamp_dir.join(input.stamp_name);
    let Ok(stamped) = fs::read(&stamp_path) else {
        return;
    };
    let current = fs::read(&input.path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", input.path.display()));
    if current != stamped {
        fail_dependencies_changed(&input.path, &stamp_path);
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
         tests/common/mod.rs loads it at run time, so nothing in this crate can be tested \
         without it.\n\
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
         tests/common/mod.rs would load the stale `.so` and silently test the OLD program.\n\
         \n\
         Run `anchor build` from the repo root, then try again.\n\n",
        so = program.so_path.display(),
        newest_path = newest_path.display(),
        label = program.source_label,
    )
}

fn fail_dependencies_changed(input_path: &Path, stamp_path: &Path) -> ! {
    panic!(
        "\n\n\
         {input} has changed since target/deploy/*.so were last built.\n\
         A dependency change alters a compiled program without touching anything under \
         `src/`, so tests/common/mod.rs would load a `.so` built against the OLD \
         dependencies.\n\
         \n\
         `diff {stamp} {input}` shows what changed.\n\
         Run `anchor build` from the repo root, then try again.\n\n",
        input = input_path.display(),
        stamp = stamp_path.display(),
    )
}
