//! Differential fuzzing, the emitting half (#164): `scripts/differential-fuzz.sh`
//! runs this, then hands every case to the reference checker.
//!
//!     cargo run --release --features write --example differential -- \
//!         FIXTURES OUT CASES [FIRST_SEED]
//!
//! For each fixture of the whole-image gate (`WI_FIXTURES`), a control --
//! the fixture as it is -- and then CASES seeds from FIRST_SEED, each the
//! gate's own case (`fuzz/shared/whole_image.rs`): one structure changed and
//! (mostly) re-stamped, or one field of one real key committed through the
//! writer. On each, this crate's verdict: open, the gate's bounded walk,
//! and the checker. The judge lays the same change over a copy of the
//! fixture and asks the reference checker; the classifier compares the two.
//!
//! Writes, under OUT:
//!
//! - `cases.tsv`: one line per case, tab-separated: id, fixture, seed,
//!   structure (what kind of thing was changed), what (the change, in
//!   words), ours (`clean`, `found` or `panicked`), signature (the kind of
//!   thing this crate found) and detail (what it said).
//! - `patches/<id>.patch`: the case's changed pages, each as a
//!   little-endian u64 byte offset, a u32 length and that many bytes. The
//!   control's is empty.
//! - `inflight.txt` while a case runs: if this process dies, the case it
//!   died on.
#![allow(dead_code, unused_imports)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

include!("../fuzz/shared/helpers.rs");
include!("../fuzz/shared/whole_image.rs");

/// Where a case's panic happened, set by the hook and read once the
/// unwind is caught: the payload alone does not say where.
static PANICKED_AT: Mutex<String> = Mutex::new(String::new());

/// This crate's verdict on one image.
struct Verdict {
    ours: &'static str,
    signature: String,
    detail: String,
}

impl Verdict {
    fn clean() -> Self {
        Verdict {
            ours: "clean",
            signature: String::new(),
            detail: String::new(),
        }
    }

    fn found(signature: String, detail: String) -> Self {
        Verdict {
            ours: "found",
            signature,
            detail,
        }
    }
}

/// An error's kind, without the numbers in its message: what a class of
/// disagreement is keyed on.
fn kind(e: &fs_bcachefs::Error) -> String {
    use fs_bcachefs::Error as E;
    match e {
        E::Io(_) => "io".into(),
        E::BadMagic { what } => format!("bad-magic({what})"),
        E::BadChecksum { what, .. } => format!("bad-checksum({what})"),
        E::Corrupt(_) => "corrupt".into(),
        E::Unsupported(_) => "unsupported".into(),
        E::NotFound(_) => "not-found".into(),
    }
}

/// Open, the gate's bounded walk, and the checker. Anything any of them
/// reports is something found; the checker's own finding names the kind
/// when it has one, being the most specific.
fn verdict(dev: &Arc<Overlay>) -> Verdict {
    let (open_err, walk_err) = match fs_bcachefs::Filesystem::open(Arc::clone(dev)) {
        Ok(fs) => (None, wi_walk(&fs)),
        Err(e) => (Some(e), None),
    };
    match fs_bcachefs::check::check(&**dev) {
        Ok(r) if !r.clean() => {
            let p = &r.problems[0];
            Verdict::found(format!("check:{}", p.kind), p.detail.clone())
        }
        Err(e) => Verdict::found(format!("check-refused:{}", kind(&e)), e.to_string()),
        Ok(_) => match (open_err, walk_err) {
            (Some(e), _) => Verdict::found(format!("open:{}", kind(&e)), e.to_string()),
            (None, Some((call, e))) => {
                Verdict::found(format!("walk-{call}:{}", kind(&e)), e.to_string())
            }
            (None, None) => Verdict::clean(),
        },
    }
}

/// What kind of thing a case changed, from `apply_case`'s description of
/// it: a structure (re-stamped, or torn), a key field committed through
/// the writer, or nothing at all.
fn structure(what: &str) -> String {
    for k in ["superblock", "jset", "node"] {
        if what.starts_with(k) {
            return if what.ends_with(", torn") {
                format!("{k}-torn")
            } else {
                k.to_string()
            };
        }
    }
    if what.ends_with("insert committed") {
        if let Some(field) = what.split(": ").nth(1).and_then(|s| s.split(' ').next()) {
            return format!("key-{field}");
        }
    }
    String::from("unchanged")
}

/// One line of `cases.tsv` has no tab or newline inside a field.
fn field(s: &str) -> String {
    let flat: String = s
        .chars()
        .map(|c| {
            if c == '\t' || c == '\n' || c == '\r' {
                ' '
            } else {
                c
            }
        })
        .collect();
    flat.chars().take(300).collect()
}

fn write_patch(path: &Path, pages: &[(u64, Vec<u8>)]) {
    let mut out = Vec::new();
    for (at, bytes) in pages {
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(bytes);
    }
    std::fs::write(path, out).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

fn payload(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| String::from("(a panic without a message)"))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 || args.len() > 4 {
        eprintln!("usage: differential FIXTURES OUT CASES [FIRST_SEED]");
        std::process::exit(2);
    }
    let fixtures = PathBuf::from(&args[0]);
    let out = PathBuf::from(&args[1]);
    let cases: u64 = args[2].parse().expect("CASES is a number");
    let first: u64 = args
        .get(3)
        .map(|s| s.parse().expect("FIRST_SEED is a number"))
        .unwrap_or(0);
    let patches = out.join("patches");
    std::fs::create_dir_all(&patches).unwrap_or_else(|e| panic!("{}: {e}", patches.display()));
    let tsv = out.join("cases.tsv");
    let mut tsv = std::fs::File::create(&tsv).unwrap_or_else(|e| panic!("{}: {e}", tsv.display()));
    let inflight = out.join("inflight.txt");

    // A panic in a case is a finding, recorded with where it happened; the
    // default hook's message would only repeat it.
    std::panic::set_hook(Box::new(|info| {
        let at = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        if let Ok(mut g) = PANICKED_AT.lock() {
            *g = at;
        }
    }));

    let (mut total, mut found, mut panicked) = (0usize, 0usize, 0usize);
    for name in WI_FIXTURES {
        let path = fixtures.join(name);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| {
            panic!(
                "{}: {e} (the fixtures are built by `chore fixtures`)",
                path.display()
            )
        });
        let base = Arc::new(bytes);
        let t = targets(&base);
        let stem = name.trim_end_matches(".img");
        let seeds = std::iter::once(None).chain((first..first + cases).map(Some));
        for seed in seeds {
            let id = match seed {
                Some(s) => format!("{stem}-{s}"),
                None => format!("{stem}-control"),
            };
            std::fs::write(&inflight, format!("{id}\n")).expect("inflight.txt");
            let dev = Overlay::new(Arc::clone(&base), false);
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let what = match seed {
                    Some(s) => apply_case(&dev, &t, s),
                    None => String::from("the fixture unchanged"),
                };
                (what, verdict(&dev))
            }));
            let (what, v) = match run {
                Ok(r) => r,
                Err(p) => {
                    let at = PANICKED_AT.lock().map(|g| g.clone()).unwrap_or_default();
                    (
                        String::from("(the case panicked)"),
                        Verdict {
                            ours: "panicked",
                            signature: format!("panic at {at}"),
                            detail: payload(&*p),
                        },
                    )
                }
            };
            write_patch(&patches.join(format!("{id}.patch")), &dev.changed_pages());
            let structure = if seed.is_none() {
                String::from("control")
            } else {
                structure(&what)
            };
            writeln!(
                tsv,
                "{id}\t{name}\t{}\t{structure}\t{}\t{}\t{}\t{}",
                seed.map(|s| s.to_string()).unwrap_or_else(|| "-".into()),
                field(&what),
                v.ours,
                field(&v.signature),
                field(&v.detail)
            )
            .expect("cases.tsv");
            total += 1;
            match v.ours {
                "found" => found += 1,
                "panicked" => panicked += 1,
                _ => {}
            }
        }
    }
    tsv.flush().expect("cases.tsv");
    let _ = std::fs::remove_file(&inflight);
    println!(
        "differential: {total} cases emitted over {} fixtures; this crate found something in \
         {found}, panicked in {panicked}",
        WI_FIXTURES.len()
    );
}
