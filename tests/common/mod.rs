//! Shared helpers for the oracle tier: where the fixtures are, and readers
//! for what the reference tools recorded about each one.
//!
//! A fixture that is not there FAILS the test, naming `chore fixtures`.
//! Nothing skips: a skipped oracle reads exactly like a passing one.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Every fixture set scripts/guest-build-fixtures.sh builds.
pub const SETS: &[&str] = &[
    "default", "lz4", "zstd", "gzip", "nocsum", "xxhash", "crc64", "block4k",
];

pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".vm-share/fixtures")
}

/// Path of a fixture file, failing the test when it is absent.
pub fn fixture(name: &str) -> PathBuf {
    let p = fixtures_dir().join(name);
    assert!(
        p.exists(),
        "{} is missing: build the fixtures with `chore fixtures` (the reference tools run in the harness VM)",
        p.display()
    );
    p
}

pub fn read_text(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).unwrap()
}

/// The reference superblock printer's report as `key -> value`, for the
/// lines before the device section, with the device section's keys
/// prefixed `dev.`.
pub fn super_report(set: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut prefix = "";
    for line in read_text(&format!("{set}.super.txt")).lines() {
        if line.starts_with("Device 0:") {
            prefix = "dev.";
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim();
            if k.is_empty() {
                continue;
            }
            out.entry(format!("{prefix}{k}"))
                .or_insert_with(|| v.trim().to_string());
        }
    }
    out
}

/// "32.0k" / "512" / "1.00M" -> bytes.
pub fn parse_size(s: &str) -> u64 {
    let s = s.trim();
    let (num, mult) = match s.chars().last() {
        Some('k') => (&s[..s.len() - 1], 1024.0),
        Some('M') => (&s[..s.len() - 1], 1024.0 * 1024.0),
        Some('G') => (&s[..s.len() - 1], 1024.0 * 1024.0 * 1024.0),
        _ => (s, 1.0),
    };
    (num.parse::<f64>().unwrap_or_else(|_| panic!("size {s:?}")) * mult).round() as u64
}

/// The bracketed choice of an option line: "none [crc32c] crc64" -> "crc32c".
pub fn selected(v: &str) -> String {
    if let (Some(a), Some(b)) = (v.find('['), v.find(']')) {
        v[a + 1..b].to_string()
    } else {
        v.trim().to_string()
    }
}

/// One entry of the tree that was fed to the reference formatter.
#[derive(Debug, Clone)]
pub struct Entry {
    pub path: String,
    pub kind: String,
    pub mode: u32,
    pub size: Option<u64>,
    pub sha256: Option<String>,
    pub target: Option<String>,
}

/// The manifest, read with a hand-rolled scan of the fixed shape
/// guest-build-fixtures.sh writes (one key per line, `indent=1`).
pub fn manifest(set: &str) -> Vec<Entry> {
    let text = read_text(&format!("{set}.json"));
    let mut out = Vec::new();
    let mut cur: Option<Entry> = None;
    for line in text.lines() {
        let t = line.trim().trim_end_matches(',');
        if t == "{" {
            cur = Some(Entry {
                path: String::new(),
                kind: String::new(),
                mode: 0,
                size: None,
                sha256: None,
                target: None,
            });
            continue;
        }
        if t == "}" {
            if let Some(e) = cur.take() {
                if !e.path.is_empty() {
                    out.push(e);
                }
            }
            continue;
        }
        let Some(e) = cur.as_mut() else { continue };
        let Some((k, v)) = t.split_once(": ") else {
            continue;
        };
        let k = k.trim_matches('"');
        let s = v.trim_matches('"').to_string();
        match k {
            "path" => e.path = s,
            "type" => e.kind = s,
            "mode" => e.mode = s.parse().unwrap(),
            "size" => e.size = Some(s.parse().unwrap()),
            "sha256" => e.sha256 = Some(s),
            "target" => e.target = Some(s),
            _ => {}
        }
    }
    assert!(!out.is_empty(), "{set}.json holds no entries");
    out
}
