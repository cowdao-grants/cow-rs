//! Asserts that every upstream commit sha cited in the cow-rs source
//! tree is also pinned in `parity/source-lock.toml`. The lock is the
//! single place reviewers can audit what versions of cow-sdk, contracts,
//! services, cow-py and app-data this SDK is conformant against; if a
//! comment references a sha the lock has never heard of, the discipline
//! breaks and so does this test.
//!
//! The check is intentionally text-based: we scan for `cowprotocol/<repo>`
//! or `cowdao-grants/<repo>` URL fragments that contain a 40-character
//! lowercase hex sha and confirm each is one of the values in the lock.
//! Adding a new upstream reference is then a two-step act: bump the lock,
//! cite the same sha in the source code.

use std::{collections::HashSet, fs, path::Path};

const LOCK_PATH: &str = "../../parity/source-lock.toml";

/// Returns every 40-char lowercase-hex sha listed in `source-lock.toml`.
fn locked_shas() -> HashSet<String> {
    let body = fs::read_to_string(LOCK_PATH).unwrap_or_else(|e| {
        panic!(
            "could not read {LOCK_PATH}: {e}; CWD={:?}",
            std::env::current_dir().ok()
        )
    });
    let mut shas = HashSet::new();
    for line in body.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("sha = \"")
            && let Some(end) = rest.find('"')
        {
            shas.insert(rest[..end].to_owned());
        }
    }
    shas
}

/// Returns every 40-char lowercase-hex sha cited inside an upstream
/// GitHub URL (`/blob/<sha>/...`, `/tree/<sha>/...`, `/commit/<sha>`)
/// pointing at `cowprotocol/<repo>` or `cowdao-grants/<repo>`, together
/// with the line on which it appeared. Scanning for URL-embedded shas
/// avoids the false positives a naïve sha scan picks up (random hex
/// constants, addresses, recon notes that quote unrelated commits).
fn shas_referenced_in(path: &Path) -> Vec<(String, usize)> {
    let body = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut hits = Vec::new();
    let prefixes = ["github.com/cowprotocol/", "github.com/cowdao-grants/"];
    let anchors = ["/blob/", "/tree/", "/commit/", "/raw/"];
    for (idx, line) in body.lines().enumerate() {
        for prefix in prefixes {
            for (host_start, _) in line.match_indices(prefix) {
                let rest = &line[host_start + prefix.len()..];
                for anchor in anchors {
                    let Some(anc_pos) = rest.find(anchor) else {
                        continue;
                    };
                    let after = &rest[anc_pos + anchor.len()..];
                    if after.len() < 40 {
                        continue;
                    }
                    let candidate = &after[..40];
                    if !candidate.bytes().all(|b| {
                        b.is_ascii_digit() || (b.is_ascii_lowercase() && b.is_ascii_hexdigit())
                    }) {
                        continue;
                    }
                    // Reject if the 41st char would extend the hex run
                    // (e.g. a 64-char keccak digest embedded by accident).
                    let next = after.as_bytes().get(40).copied().map_or(b' ', |b| b);
                    if (next as char).is_ascii_hexdigit() {
                        continue;
                    }
                    hits.push((candidate.to_owned(), idx + 1));
                }
            }
        }
    }
    hits
}

/// Walk every `.rs` and `.md` file under `roots` (skipping `target/`).
fn walk_sources<F: FnMut(&Path)>(roots: &[&str], mut visit: F) {
    let mut stack: Vec<std::path::PathBuf> = roots.iter().map(Into::into).collect();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if matches!(
                    name,
                    "target"
                        | ".git"
                        | "node_modules"
                        | "pkg"
                        | "pkg-web"
                        | "pkg-bundler"
                        | "pkg-nodejs"
                ) {
                    continue;
                }
                stack.push(path);
            } else {
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                if matches!(ext, "rs" | "md" | "toml") {
                    visit(&path);
                }
            }
        }
    }
}

#[test]
fn lock_contains_known_repositories() {
    let body = fs::read_to_string(LOCK_PATH).expect("source-lock.toml is readable");
    for required in &[
        "cow-sdk-ts",
        "cow-sdk-ts-pr-867",
        "contracts",
        "services",
        "cow-py",
        "app-data",
    ] {
        assert!(
            body.contains(&format!("name = \"{required}\"")),
            "parity/source-lock.toml is missing source {required:?}",
        );
    }
}

#[test]
fn every_sha_in_lock_is_40_hex() {
    let shas = locked_shas();
    assert!(!shas.is_empty(), "lock has no sha entries");
    for sha in &shas {
        assert_eq!(sha.len(), 40, "sha {sha:?} is not 40 chars");
        assert!(
            sha.bytes()
                .all(|b| b.is_ascii_digit() || (b.is_ascii_lowercase() && b.is_ascii_hexdigit())),
            "sha {sha:?} is not lowercase hex",
        );
    }
}

/// Catches a comment in our own source code that names a sha the lock
/// has never heard of. The check is scoped to files inside our SDK
/// crates and the `recon/` notes; vendored Cargo.lock-style fixtures
/// would otherwise produce noise.
#[test]
fn every_sha_cited_in_source_is_pinned_in_lock() {
    let locked = locked_shas();
    let mut violations: Vec<String> = Vec::new();
    walk_sources(
        &["../../crates", "../../recon", "../../README.md"],
        |path| {
            // Don't audit the lock itself.
            if path.ends_with("source-lock.toml") {
                return;
            }
            for (sha, line) in shas_referenced_in(path) {
                if !locked.contains(&sha) {
                    violations.push(format!("{}:{line}: {sha}", path.display()));
                }
            }
        },
    );
    assert!(
        violations.is_empty(),
        "the following sha references are not pinned in parity/source-lock.toml:\n{}",
        violations.join("\n"),
    );
}
