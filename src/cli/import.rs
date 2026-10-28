//! `fs.bcachefs import`: stream one local file or stdin into the image
//! through rust-fs-core's shared import, publishing it only when it is
//! complete (#439). This file is the shared sink over the library's
//! `Writer`, every commit of which goes through the journal:
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

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::cli::transfer::{self, FileSink};
use fs_core::cli::{CliError, Outcome};

use super::device::{Device, Target};
use super::fs_write::{failed, write_path};

/// Set in the guest's test (tests/write_oracle.rs) to stop the tool after
/// persist, so that it can be killed before publish.
const HOLD_AFTER_PERSIST: &str = "FS_BCACHEFS_IMPORT_HOLD_AFTER_PERSIST";

/// How much is gathered before the library's write is called. Each call
/// rewrites the file's stored blocks (#441), so fewer, larger calls cost
/// less; this bounds what is held besides.
const GATHER: usize = 16 << 20;

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

    fn flush(&mut self) -> Result<(), CliError> {
        if self.gathered.is_empty() {
            return Ok(());
        }
        let ino = self
            .staged
            .as_ref()
            .map(|s| s.ino)
            .ok_or_else(|| CliError::failed("write before stage"))?;
        let at = self.gathered_at;
        let bytes = std::mem::take(&mut self.gathered);
        self.writer()?.write_at(ino, at, &bytes).map_err(failed)
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
            self.flush()?;
        }
        if self.gathered.is_empty() {
            self.gathered_at = offset;
        }
        self.gathered.extend_from_slice(bytes);
        if self.gathered.len() >= GATHER {
            self.flush()?;
        }
        Ok(())
    }

    fn persist(&mut self) -> Result<(), CliError> {
        self.flush()?;
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
        let Some(staged) = self.staged.take() else {
            return Ok(());
        };
        // A writer a failed commit left is not trusted with the unlink.
        self.writer = None;
        let unlinked = self
            .writer()
            .and_then(|w| w.unlink(staged.dir, &staged.stage).map_err(failed));
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

/// Run `import`.
pub fn run(target: &Target<'_>, image: &OsString, sub: &ArgMatches) -> Result<Outcome, CliError> {
    let request = transfer::ImportRequest::from_matches(sub)?;
    let options = transfer::ImportOptions::from_matches(sub)?;
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
    transfer::import_with_options(&request, options, Some(&mut sink))
}
