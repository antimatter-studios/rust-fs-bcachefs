//! `fs.bcachefs import`: stream a local file, stdin or a local tree into the
//! image through rust-fs-core's shared import, publishing each file only when
//! it is complete (#439, #440). This file is the shared sink over the
//! library's `Writer`, every commit of which goes through the journal:
//!
//! - **stage**: an empty file with a private name,
//!   `.fs-bcachefs-import-PID-N.partial`, is created in the destination's
//!   directory;
//! - **write**: bytes go in at increasing offsets through the library's
//!   positioned write, gathered into bounded runs;
//! - **persist**: the last run is committed; each commit is one journal
//!   entry on the device before the call returns;
//! - **read back**: for `--verify`, through a fresh read-only open of the
//!   image, never through the writer;
//! - **publish**: one rename of the staged name to the final name. The
//!   rename replaces an existing name in the same transaction, and that
//!   transaction is one journal entry, which is why `--replace` is offered.
//!   The old file is never deleted first;
//! - **abort**: the staged name is unlinked. The final name is not touched.
//!
//! A crash between persist and publish leaves the old file (or no file)
//! under the final name and the staged name beside it: every commit is a
//! whole journal entry or none (src/write_journal.rs, CRASH SAFETY), so the
//! image is consistent once replayed, and the leftover is an ordinary file
//! that `rm` removes.
//!
//! A tree adds two commits of its own: a directory is one `mkdir`, and an
//! extended attribute under `--preserve=xattrs` (#443) is one `set_xattr` on
//! the item already published. Each is whole or absent after a crash, so a
//! tree cut short is the items that were finished, each of them intact.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use clap::{Arg, ArgMatches};
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::cli::transfer::{self, ExtendedAttribute, FileSink, TreeSink};
use fs_core::cli::{CliError, Outcome};

use super::device::{Device, Target};
use super::fs_write::{failed, write_path};

/// Set in the guest's test (tests/write_oracle.rs) to stop the tool after
/// persist, so that it can be killed before publish.
const HOLD_AFTER_PERSIST: &str = "FS_BCACHEFS_IMPORT_HOLD_AFTER_PERSIST";

/// How much is gathered before the library's write is called. Each call
/// rewrites the file's stored blocks (#441), so fewer, larger calls cost
/// less; this bounds what is held besides. A call is one commit, so it is
/// also no longer than one journal entry holds (#485,
/// `Writer::largest_journalled_write`).
const GATHER: usize = 16 << 20;

/// The permission bits of a directory a tree import makes: `mkdir`'s default.
const DIR_MODE: u32 = 0o755;

/// The longest name a bcachefs directory entry holds, in bytes.
const NAME_MAX: usize = 255;

/// The arguments `import` takes: the shared tree set, and `--verify`.
pub fn args() -> [Arg; 7] {
    let [source, path, replace, recursive, depth, preserve] = transfer::import_args_with_xattrs();
    let [_, _, _, verify] = transfer::import_args_with_verify();
    [
        source,
        path,
        replace,
        recursive,
        depth,
        preserve,
        verify.help(
            "Read the persisted staged file back and verify its length and SHA-256 \
             before publishing (one file, not --recursive or --preserve)",
        ),
    ]
}

/// A name this sink can store: bcachefs's rules (not empty, `.` or `..`, no
/// `/` or NUL, at most 255 bytes), and UTF-8, the one spelling the paths this
/// tool resolves take.
fn checked_name(name: &OsStr) -> Result<&str, CliError> {
    let Some(text) = name.to_str() else {
        return Err(CliError::refused(format!(
            "{name:?}: this tool writes names in UTF-8 only"
        )));
    };
    if text.is_empty() || text == "." || text == ".." {
        return Err(CliError::refused(format!(
            "{text:?}: not a usable name in a directory"
        )));
    }
    if text.len() > NAME_MAX {
        return Err(CliError::refused(format!(
            "{text:?}: a name of {} bytes, and bcachefs names hold at most {NAME_MAX}",
            text.len()
        )));
    }
    if text.contains(['/', '\0']) {
        return Err(CliError::refused(format!(
            "{text:?}: bcachefs names hold no NUL and no /"
        )));
    }
    Ok(text)
}

/// A path as this tool's lookups take it, without a trailing `/`; the root
/// is `/`.
fn image_path(path: &Path) -> Result<&str, CliError> {
    let text = path
        .to_str()
        .ok_or_else(|| CliError::refused(format!("{path:?}: this tool writes UTF-8 paths only")))?;
    let trimmed = text.trim_end_matches('/');
    Ok(if trimmed.is_empty() { "/" } else { trimmed })
}

/// The file being staged: where it is, and what it will be called.
struct Staged {
    dir: u64,
    stage: Vec<u8>,
    name: Vec<u8>,
    ino: u64,
    path: String,
}

struct Sink<'a> {
    target: &'a Target<'a>,
    image: PathBuf,
    writer: Option<Writer<Device>>,
    reader: Option<Filesystem<Device>>,
    staged: Option<Staged>,
    counter: u32,
    gathered: Vec<u8>,
    gathered_at: u64,
}

impl Sink<'_> {
    fn writer(&mut self) -> Result<&mut Writer<Device>, CliError> {
        if self.writer.is_none() {
            let dev = self.target.open_rw()?;
            self.writer = Some(Writer::open_journalled(dev).map_err(failed)?);
        }
        Ok(self.writer.as_mut().expect("opened above"))
    }

    /// Commit the first `len` gathered bytes.
    fn commit_run(&mut self, len: usize) -> Result<(), CliError> {
        let ino = self
            .staged
            .as_ref()
            .map(|s| s.ino)
            .ok_or_else(|| CliError::failed("write before stage"))?;
        let at = self.gathered_at;
        let bytes: Vec<u8> = self.gathered.drain(..len).collect();
        self.gathered_at += len as u64;
        self.writer()?.write_at(ino, at, &bytes).map_err(failed)
    }

    /// Commit whole runs of the gathered bytes, and with `all` the rest.
    fn flush(&mut self, all: bool) -> Result<(), CliError> {
        if self.gathered.is_empty() {
            return Ok(());
        }
        let largest = self
            .writer()?
            .largest_journalled_write()
            .map_err(failed)?
            .min(GATHER);
        while self.gathered.len() >= largest {
            self.commit_run(largest)?;
        }
        if all && !self.gathered.is_empty() {
            self.commit_run(self.gathered.len())?;
        }
        Ok(())
    }
}

impl FileSink for Sink<'_> {
    fn image_path(&self) -> Option<&Path> {
        Some(&self.image)
    }

    fn supports_atomic_replace(&self) -> bool {
        true
    }

    fn stage(&mut self, path: &Path, replace: bool) -> Result<(), CliError> {
        let p = path
            .to_str()
            .ok_or_else(|| CliError::usage("PATH is not UTF-8"))?
            .to_string();
        let (dir, name, existing) = write_path(self.target, &p, true)?;
        if existing.is_some() && !replace {
            return Err(CliError::failed(format!(
                "{p}: exists; give --replace to replace it"
            )));
        }
        self.counter += 1;
        let stage = format!(
            ".fs-bcachefs-import-{}-{}.partial",
            std::process::id(),
            self.counter
        )
        .into_bytes();
        let ino = self
            .writer()?
            .create_file(dir, &stage, &[], 0o644)
            .map_err(failed)?;
        self.staged = Some(Staged {
            dir,
            stage,
            name,
            ino,
            path: p,
        });
        Ok(())
    }

    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<(), CliError> {
        if self.staged.is_none() {
            return Err(CliError::failed("write_at before stage"));
        }
        if !self.gathered.is_empty() && self.gathered_at + self.gathered.len() as u64 != offset {
            self.flush(true)?;
        }
        if self.gathered.is_empty() {
            self.gathered_at = offset;
        }
        self.gathered.extend_from_slice(bytes);
        self.flush(false)
    }

    fn persist(&mut self) -> Result<(), CliError> {
        self.flush(true)?;
        self.writer = None;
        if std::env::var_os(HOLD_AFTER_PERSIST).is_some() {
            eprintln!("fs.bcachefs import: held after persist");
            loop {
                std::thread::park();
            }
        }
        Ok(())
    }

    fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> Result<usize, CliError> {
        let ino = self
            .staged
            .as_ref()
            .map(|s| s.ino)
            .ok_or_else(|| CliError::failed("read_at before stage"))?;
        if self.reader.is_none() {
            self.reader = Some(Filesystem::open(self.target.open()?).map_err(failed)?);
        }
        let fs = self.reader.as_ref().expect("opened above");
        let data = fs.read_range(ino, offset, bytes.len()).map_err(failed)?;
        bytes[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }

    fn publish(&mut self, replace: bool) -> Result<bool, CliError> {
        self.reader = None;
        let staged = self
            .staged
            .take()
            .ok_or_else(|| CliError::failed("publish before stage"))?;
        let fs = Filesystem::open(self.target.open()?).map_err(failed)?;
        let existing = match fs.lookup(&staged.path) {
            Ok(ino) => Some(ino),
            Err(fs_bcachefs::Error::NotFound(_)) => None,
            Err(e) => {
                self.staged = Some(staged);
                return Err(failed(e));
            }
        };
        drop(fs);
        if existing.is_some() && !replace {
            let path = staged.path.clone();
            self.staged = Some(staged);
            return Err(CliError::failed(format!(
                "{path}: exists; give --replace to replace it"
            )));
        }
        let renamed = self.writer().and_then(|w| {
            w.rename(staged.dir, &staged.stage, staged.dir, &staged.name)
                .map_err(failed)
        });
        if let Err(e) = renamed {
            self.staged = Some(staged);
            return Err(e);
        }
        self.writer = None;
        Ok(existing.is_some())
    }

    fn abort(&mut self) -> Result<(), CliError> {
        self.gathered.clear();
        self.reader = None;
        // A writer a failed commit left is not trusted with the unlink, or,
        // in a tree, with the next item.
        self.writer = None;
        let Some(staged) = self.staged.take() else {
            return Ok(());
        };
        let unlinked = self
            .writer()
            .and_then(|w| w.unlink_in_steps(staged.dir, &staged.stage).map_err(failed));
        self.writer = None;
        unlinked.map_err(|e| {
            CliError::failed(format!(
                "{}: the staged file {} was not removed: {}",
                staged.path,
                String::from_utf8_lossy(&staged.stage),
                e.message
            ))
        })
    }
}

impl TreeSink for Sink<'_> {
    fn validate_name(&mut self, _parent: &Path, name: &OsStr) -> Result<OsString, CliError> {
        checked_name(name)?;
        Ok(name.to_os_string())
    }

    fn create_dir(&mut self, path: &Path) -> Result<bool, CliError> {
        let p = image_path(path)?;
        let fs = Filesystem::open(self.target.open()?).map_err(failed)?;
        match fs.lookup(p) {
            Ok(ino) => {
                // The lookup does not follow a last-component symlink, so a
                // link is refused here rather than written through.
                if fs.inode(ino).map_err(failed)?.is_dir() {
                    return Ok(true);
                }
                return Err(CliError::refused_by_state(format!(
                    "{p}: exists and is not a directory"
                )));
            }
            Err(fs_bcachefs::Error::NotFound(_)) => {}
            Err(e) => return Err(failed(e)),
        }
        drop(fs);
        // The same checks a staged file gets: a missing or casefolded parent,
        // a snapshotted root, options the writer cannot honour.
        let (dir, name, _) = write_path(self.target, p, true)?;
        let made = self
            .writer()
            .and_then(|w| w.mkdir(dir, &name, DIR_MODE).map_err(failed));
        self.writer = None;
        made.map(|_| false)
    }

    fn format_name(&self) -> &str {
        "bcachefs"
    }

    fn set_extended_attribute(
        &mut self,
        path: &Path,
        attribute: &ExtendedAttribute,
    ) -> Result<(), CliError> {
        let p = image_path(path)?;
        let ino = Filesystem::open(self.target.open()?)
            .map_err(failed)?
            .lookup(p)
            .map_err(failed)?;
        // The library refuses a namespace it does not write, and a name or
        // value one key cannot hold: exit 3, so the item reads as refused.
        let set = self.writer().and_then(|w| {
            w.set_xattr(ino, attribute.name.as_encoded_bytes(), &attribute.value)
                .map_err(failed)
        });
        self.writer = None;
        set
    }
}

/// Run `import`.
pub fn run(target: &Target<'_>, image: &OsString, sub: &ArgMatches) -> Result<Outcome, CliError> {
    let request = transfer::ImportRequest::from_matches(sub)?;
    let options = transfer::ImportOptions::from_matches(sub)?;
    let tree = transfer::ImportTreeOptions::from_matches(sub)?;
    let sparse = transfer::SparseOptions::from_matches(sub)?;
    let xattrs = transfer::XattrOptions::from_matches(sub)?;
    // One file under --preserve goes through the tree transfer and gets its
    // report, as export does: the single-file path has no metadata to carry.
    let through_tree = tree.recursive
        || tree.preserve_links
        || tree.preserve_times
        || sparse.preserve_sparse
        || xattrs.preserve_xattrs;
    if through_tree && options.verify {
        return Err(CliError::usage(
            "--verify reads one staged file back; it does not apply to --recursive or --preserve",
        ));
    }
    // This sink keeps no holes, and rust-fs-core's sparse tree path takes no
    // attributes: every file would be refused and every directory's attributes
    // lost, so the pair is refused whole.
    if sparse.preserve_sparse && xattrs.preserve_xattrs {
        return Err(CliError::refused(
            "--preserve=sparse: sparse files not supported by bcachefs, so nothing was imported",
        ));
    }
    let mut sink = Sink {
        target,
        image: PathBuf::from(image),
        writer: None,
        reader: None,
        staged: None,
        counter: 0,
        gathered: Vec::new(),
        gathered_at: 0,
    };
    if !through_tree {
        return transfer::import_with_options(&request, options, Some(&mut sink));
    }
    if sparse.preserve_sparse {
        return transfer::import_tree_with_sparse(&request, Some(&mut sink), tree, sparse);
    }
    transfer::import_tree_with_xattrs(&request, Some(&mut sink), tree, xattrs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(name: &OsStr) -> bool {
        matches!(checked_name(name), Err(e) if e.code == fs_core::cli::output::EXIT_UNSUPPORTED)
    }

    #[test]
    fn a_name_bcachefs_cannot_store_is_refused() {
        for name in ["", ".", "..", "a/b", "a\0b"] {
            assert!(refused(OsStr::new(name)), "{name:?}");
        }
        assert!(refused(OsStr::new(&"n".repeat(NAME_MAX + 1))));
        // Three bytes each: 256 characters is the limit in neither unit.
        assert!(refused(OsStr::new(&"\u{65e5}".repeat(86))));
    }

    #[cfg(unix)]
    #[test]
    fn a_name_that_is_not_utf8_is_refused() {
        use std::os::unix::ffi::OsStrExt;
        assert!(refused(OsStr::from_bytes(b"bad\xffname")));
    }

    #[test]
    fn a_name_bcachefs_can_store_is_taken_as_it_is() {
        let longest = "n".repeat(NAME_MAX);
        for name in [
            "a",
            "...",
            ".hidden",
            "a name",
            "\u{65e5}\u{672c}",
            &longest,
        ] {
            assert_eq!(checked_name(OsStr::new(name)).unwrap(), name);
        }
    }

    #[test]
    fn a_path_loses_its_trailing_slash_and_the_root_stays() {
        for (given, want) in [("/", "/"), ("//", "/"), ("/d/", "/d"), ("/d/e", "/d/e")] {
            assert_eq!(image_path(Path::new(given)).unwrap(), want);
        }
    }
}
