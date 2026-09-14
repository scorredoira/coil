use std::borrow::Cow;
use std::path::Path;
use std::process::Command;

const MAJOR: &str = env!("CARGO_PKG_VERSION_MAJOR");
const MINOR: &str = env!("CARGO_PKG_VERSION_MINOR");
const PATCH: &str = env!("CARGO_PKG_VERSION_PATCH");

fn main() {
    let git_hash = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|x| String::from_utf8(x.stdout).ok())
        .or_else(|| option_env!("HELIX_NIX_BUILD_REV").map(|s| s.to_string()));

    let minor = if MINOR.len() == 1 {
        // Print single-digit months in '0M' format
        format!("0{MINOR}")
    } else {
        MINOR.to_string()
    };
    let calver = if PATCH == "0" {
        format!("{MAJOR}.{minor}")
    } else {
        format!("{MAJOR}.{minor}.{PATCH}")
    };
    // sid's releases are tags like v2026.9.17: name the build after the release
    // it is, or the one it follows ("v2026.9.17+2" is two commits past it).
    let calver = sid_release().unwrap_or(calver);
    let version: Cow<_> = match &git_hash {
        Some(git_hash) => format!("{} ({})", calver, &git_hash[..8]).into(),
        None => calver.into(),
    };

    println!(
        "cargo:rustc-env=BUILD_TARGET={}",
        std::env::var("TARGET").unwrap()
    );

    println!("cargo:rustc-env=VERSION_AND_GIT_HASH={}", version);

    if git_hash.is_none() {
        return;
    }

    // we need to revparse because the git dir could be anywhere if you are
    // using detached worktrees but there is no good way to obtain an OsString
    // from command output so for now we can't accept non-utf8 paths here
    // probably rare enough where it doesn't matter tough we could use gitoxide
    // here but that would be make it a hard dependency and slow compile times
    let Some(git_dir): Option<String> = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|x| String::from_utf8(x.stdout).ok())
        .map(|x| x.trim().to_string())
    else {
        return;
    };
    // If heads starts pointing at something else (different branch)
    // we need to return
    let head = Path::new(&git_dir).join("HEAD");
    if head.exists() {
        println!("cargo:rerun-if-changed={}", head.display());
    }
    // if the thing head points to (branch) itself changes
    // we need to return
    let Some(head_ref): Option<String> = Command::new("git")
        .args(["symbolic-ref", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|x| String::from_utf8(x.stdout).ok())
        .map(|x| x.trim().to_string())
    else {
        return;
    };
    let head_ref = Path::new(&git_dir).join(head_ref);
    if head_ref.exists() {
        println!("cargo:rerun-if-changed={}", head_ref.display());
    }
    // A branch may live only in packed-refs, and a new tag renames the build;
    // the HEAD log moves on every commit, checkout and reset.
    for watched in ["packed-refs", "refs/tags", "logs/HEAD"] {
        let path = Path::new(&git_dir).join(watched);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

fn sid_release() -> Option<String> {
    println!("cargo:rerun-if-env-changed=GITHUB_REF_TYPE");
    println!("cargo:rerun-if-env-changed=GITHUB_REF_NAME");
    // A release build checks out the tag shallowly, where describe may not see it.
    if std::env::var("GITHUB_REF_TYPE").as_deref() == Ok("tag") {
        return std::env::var("GITHUB_REF_NAME").ok();
    }
    let describe = Command::new("git")
        .args(["describe", "--tags", "--match", "v[0-9]*"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|x| String::from_utf8(x.stdout).ok())?;
    let describe = describe.trim();
    // "v2026.9.17-2-g3c25a543" → "v2026.9.17+2"
    Some(match describe.rsplitn(3, '-').collect::<Vec<_>>()[..] {
        [_hash, ahead, tag] if ahead.parse::<u32>().is_ok() => format!("{tag}+{ahead}"),
        _ => describe.to_string(),
    })
}
