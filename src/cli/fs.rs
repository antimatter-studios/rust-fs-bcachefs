//! `fs.bcachefs`: inspect a bcachefs image, and (experimental, src/cli/
//! fs_write.rs) change it.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use clap::{value_parser, Arg, ArgAction, ArgMatches, Command as Cmd};

use super::device::{Device, Target};
use fs_bcachefs::inode::{Dirent, Inode, INODE_HAS_CASE_INSENSITIVE};
use fs_bcachefs::superblock::{format_uuid, Superblock};
use fs_bcachefs::{fs::Node, Filesystem};
use fs_core::cli::{transfer, CliError, Format, Json, Outcome, Tool};

pub const TOOL: Tool = Tool {
    name: "fs.bcachefs",
    verb: "fs",
    section: 1,
    usage_exit: fs_core::cli::output::EXIT_USAGE,
    about: "Inspect a bcachefs image or device read-only, without mounting it",
    command,
    run,
};

const KEYS: &[&str] = &[
    "fs",
    "label",
    "total_bytes",
    "free_bytes",
    "block_size",
    "dirty",
    "bcachefs",
];

fn command() -> Cmd {
    super::fs_write::subcommands(read_command())
}

fn read_command() -> Cmd {
    Cmd::new("fs.bcachefs")
        .about("Inspect a bcachefs image or device read-only, without mounting it")
        .arg(
            Arg::new("target")
                .value_name("TARGET")
                .help("The image file or device")
                .value_parser(value_parser!(OsString))
                .required(true),
        )
        .arg(
            Arg::new("offset")
                .long("offset")
                .value_name("BYTES")
                .value_parser(value_parser!(u64))
                .global(true)
                .help("Byte offset of the filesystem in TARGET (default 0), for a partition inside a whole-disk image"),
        )
        .after_help(
            "--offset applies to the filesystem device on every verb, including experimental writes; it never seeks within a file.\n\nExamples:\n  fs.bcachefs --offset 1048576 whole-disk.img info\n  fs.bcachefs whole-disk.img read /hello.txt --offset 1048576\n  fs.bcachefs whole-disk.img write /hello.txt --from hello.txt --offset 1048576",
        )
        .args(fs_core::cli::format_args().map(|a| a.global(true)))
        .subcommand_required(true)
        .subcommand(key_command("get"))
        .subcommand(key_command("info"))
        .subcommand(
            Cmd::new("set")
                .about("Change a property (not implemented)")
                .arg(Arg::new("key").value_name("KEY").required(true))
                .arg(Arg::new("value").value_name("VALUE").required(true))
                .after_help("Example:\n  fs.bcachefs disk.img set label BACKUP\n\nAnswers not implemented (exit 3), before opening the target."),
        )
        .subcommand(
            Cmd::new("resize")
                .about("Grow or shrink the filesystem (not implemented)")
                .arg(Arg::new("size").value_name("SIZE").required(true).allow_hyphen_values(true))
                .arg(Arg::new("force").long("force").action(ArgAction::SetTrue).help("Do it without asking"))
                .after_help("Example:\n  fs.bcachefs disk.img resize 20G --force\n\nAnswers not implemented (exit 3), before opening the target."),
        )
        .subcommand(listing_command("ls"))
        .subcommand(
            Cmd::new("stat")
                .about("Report one path: identity, type, mode, owner, links, sizes and every time")
                .arg(
                    Arg::new("path")
                        .value_name("PATH")
                        .value_parser(value_parser!(OsString))
                        .required(true),
                )
                .after_help(
                    "Examples:\n  fs.bcachefs disk.img stat /etc/hostname\n  fs.bcachefs disk.img stat /var/log | jq .nlink\n  fs.bcachefs disk.img stat --text /bin/sh\n\nA symlink is reported, not followed. The shared keys come first, in this order: path, inode, type, mode, uid, gid, nlink, size, allocated_bytes, then atime, mtime, ctime and btime (the creation time), each as signed seconds since the epoch with *_nsec beside it. A non-UTF-8 PATH is resolved as bytes and adds path_hex; a symlink adds target; a device node adds major and minor, null because the driver does not decode its device number (bcachefs.dev holds it as stored). bcachefs holds snapshot, sectors, flags (by name, unknown bits as one hex value), generation, and the data_checksum, compression and background_compression options the inode carries (null when it carries none). The ino, sectors and *_ns keys of earlier releases are gone: inode, bcachefs.sectors and the seconds plus nanoseconds replace them.",
                ),
        )
        .subcommand(listing_command("tree"))
        .subcommand(
            Cmd::new("export")
                .about("Copy a regular file, or with --recursive a directory tree")
                .args(transfer::export_args_with_xattrs())
                .after_help(
                    "Examples:\n  fs.bcachefs disk.img export /hello.txt hello.txt\n  \
                     fs.bcachefs disk.img export --replace /big/random.bin random.bin\n  \
                     fs.bcachefs disk.img export /hello.txt -\n  \
                     fs.bcachefs disk.img export --recursive /etc etc\n  \
                     fs.bcachefs disk.img export --recursive --preserve=links,times /etc etc\n  \
                     fs.bcachefs disk.img export --recursive --preserve=xattrs /home home\n  \
                     fs.bcachefs disk.img export --preserve=xattrs /hello.txt hello.txt\n\n\
                     Files are copied in fixed chunks. A local destination is published only \
                     when complete; an existing name is refused unless --replace is given. \
                     The report includes the byte count and SHA-256; DEST - writes only raw \
                     bytes to stdout. Without --recursive directories, symlinks and special \
                     files are refused.\n\n\
                     Recursive export uses the shared tree transfer and reports every item; \
                     subvolumes and snapshots are entered as ls --recursive enters them. \
                     Existing directories are merged into; existing files require --replace. \
                     --max-depth bounds the walk. Special files are skipped. Symlinks require \
                     --preserve=links, which recreates their targets without following them \
                     and copies each regular inode once. --preserve=times sets the stored \
                     access and modification times. Symlink times are not set. --verify reads \
                     each staged file back before publication.\n\n\
                     --preserve=xattrs carries each file's and directory's extended attributes, \
                     name and value, and needs a local DEST; a single file goes through the \
                     tree transfer and gets its report. An attribute the host cannot hold, \
                     such as trusted.* without privilege, refuses its item by name after the \
                     contents are published, and the export exits nonzero; nothing is dropped.",
                ),
        )
        .subcommand(
            Cmd::new("read")
                .visible_alias("cat")
                .about("Read a regular file's bytes; refuse symlinks, naming their target")
                .arg(Arg::new("path").value_name("PATH").required(true))
                .arg(
                    Arg::new("output")
                        .short('o')
                        .long("output")
                        .value_name("FILE")
                        .value_parser(value_parser!(OsString))
                        .help("Write FILE.partial and rename to FILE on success, instead of stdout"),
                )
                .after_help(
                    "Examples:\n  fs.bcachefs disk.img read /hello.txt\n  fs.bcachefs disk.img read /hello.txt -o hello.txt",
                ),
        )
}

fn listing_command(name: &'static str) -> Cmd {
    let bound = |id: &'static str, help: &'static str| {
        let arg = Arg::new(id)
            .long(id)
            .value_name("N")
            .value_parser(value_parser!(u64))
            .help(help);
        if name == "ls" {
            arg.requires("recursive")
        } else {
            arg
        }
    };
    Cmd::new(name)
        .about(if name == "ls" { "List a directory: name, type, inode, size" } else { "Alias of ls --recursive" })
        .arg(Arg::new("recursive").long("recursive").short('R').action(ArgAction::SetTrue)
            .help("Walk depth first, names sorted as bytes; never follow symlinks"))
        .arg(bound("max-depth", "Maximum recursive depth (default: 64); PATH's entries are depth 1"))
        .arg(bound("limit", "Maximum recursive entries (default: 100000)"))
        .arg(Arg::new("path").value_name("PATH").value_parser(value_parser!(OsString)).default_value("/"))
        .after_help("Examples:\n  fs.bcachefs disk.img ls /etc\n  fs.bcachefs disk.img ls -R --max-depth 8 --limit 1000 /etc\n  fs.bcachefs disk.img tree /\n\nRecursive entries add volume-absolute path and relative depth. PATH itself is omitted unless it is not a directory (depth 0). Subvolume and snapshot roots carry subvolume and snapshot IDs. Repeated directory identities carry revisited: true and are not entered. Exact fits succeed; one more entry past either bound, or an unreadable directory, keeps the collected report on stdout and names the bound or path on stderr (exit 1). --limit 0 allows no entries; --max-depth 0 none below PATH.")
}

fn key_command(name: &'static str) -> Cmd {
    Cmd::new(name)
        .about("Report every filesystem property, or one KEY; get and info are identical")
        .arg(Arg::new("key").value_name("KEY").help("A shared key or bcachefs.<field>"))
        .after_help(format!(
            "Examples:\n  fs.bcachefs disk.img {name}\n  fs.bcachefs disk.img {name} label --text\n  fs.bcachefs disk.img {name} bcachefs.uuid\n\nShared keys: {}. Empty labels are null. Filesystem-specific fields are nested under bcachefs.",
            KEYS.join(", ")
        ))
}

fn run(m: &ArgMatches) -> Result<Outcome, CliError> {
    let target = m.get_one::<OsString>("target").expect("required");
    let offset = m
        .subcommand()
        .and_then(|(_, sub)| sub.get_one::<u64>("offset"))
        .or_else(|| m.get_one::<u64>("offset"))
        .copied()
        .unwrap_or(0);
    // Unsupported operations still refuse without opening any target.
    if let Some((verb @ ("set" | "resize"), _)) = m.subcommand() {
        return Err(CliError::not_implemented(format!(
            "{verb}: not implemented by this driver"
        )));
    }
    let target = Target::new(target, offset);
    // Validate the window before dispatch, including every write preflight.
    let dev = target.open()?;
    if let Some((verb, sub)) = m.subcommand() {
        if let Some(r) = super::fs_write::run(&target, verb, sub) {
            return r;
        }
    }
    match m.subcommand() {
        Some(("get" | "info", sub)) => get(
            &open(dev)?,
            sub.get_one::<String>("key").map(String::as_str),
        ),
        Some((verb @ ("ls" | "tree"), sub)) if verb == "tree" || sub.get_flag("recursive") => {
            ls_recursive(
                &open(dev)?,
                sub,
                Format::of(m, &std::env::args_os().collect::<Vec<_>>()),
            )
        }
        Some(("ls", sub)) => {
            let fs = open(dev)?;
            let path = sub.get_one::<OsString>("path").expect("defaulted");
            let path = path
                .to_str()
                .ok_or_else(|| CliError::usage("PATH is not UTF-8"))?;
            let ino = fs.lookup(path).map_err(failed)?;
            let mut rows = Vec::new();
            for d in fs.readdir(ino).map_err(failed)? {
                let size = fs.inode(d.inum).map(|i| i.size).unwrap_or(0);
                rows.push(ls_entry(&d.name, d.type_name(), d.inum, size));
            }
            Ok(Outcome::report(Json::Arr(rows)))
        }
        Some(("stat", sub)) => stat(
            &open(dev)?,
            sub.get_one::<OsString>("path")
                .expect("required")
                .as_bytes(),
        ),
        Some(("export", sub)) => {
            let request = transfer::ExportRequest::from_matches(sub)?;
            let tree = transfer::TreeOptions::from_matches(sub)?;
            let xattrs = transfer::XattrOptions::from_matches(sub)?;
            let fs = open(dev)?;
            // Attributes need a published host file to land on, which only
            // the tree transfer gives, a single file included.
            if tree.recursive || xattrs.preserve_xattrs {
                let image = m.get_one::<OsString>("target").expect("required");
                transfer::export_tree_with_xattrs(
                    &request,
                    &mut TreeExport {
                        fs: &fs,
                        image: Path::new(image),
                        ids: HashMap::new(),
                        last: None,
                    },
                    tree,
                    transfer::LinkOptions::from_matches(sub)?,
                    transfer::TimeOptions::from_matches(sub)?,
                    xattrs,
                )
            } else {
                export(&fs, &request, transfer::ExportOptions::from_matches(sub)?)
            }
        }
        Some(("read", sub)) => {
            let fs = open(dev)?;
            let path = sub.get_one::<String>("path").expect("required");
            read(&fs, path, sub.get_one("output"))
        }
        _ => Err(CliError::usage("unknown subcommand")),
    }
}

fn get(fs: &Filesystem<Device>, key: Option<&str>) -> Result<Outcome, CliError> {
    let sb = fs.superblock();
    let usage = fs.usage().map_err(failed)?;
    let label = sb.label_str();
    let all = Json::object([
        ("fs", Json::Str("bcachefs".into())),
        (
            "label",
            if label.is_empty() {
                Json::Null
            } else {
                Json::Str(label)
            },
        ),
        ("total_bytes", Json::UInt(usage.total_bytes)),
        ("free_bytes", Json::UInt(usage.free_bytes)),
        ("block_size", Json::UInt(u64::from(sb.block_size) * 512)),
        ("dirty", Json::Bool(!sb.is_clean())),
        (
            "bcachefs",
            Json::object([
                ("uuid", Json::Str(format_uuid(&sb.user_uuid))),
                (
                    "version",
                    Json::Str(format!("{}.{}", sb.version.major(), sb.version.minor())),
                ),
                (
                    "btree_node_size",
                    Json::UInt(u64::from(sb.btree_node_size()) * 512),
                ),
                ("devices", Json::UInt(u64::from(sb.nr_devices))),
                ("encrypted", Json::Bool(sb.is_encrypted())),
                ("clean", Json::Bool(sb.is_clean())),
            ]),
        ),
    ]);
    let Some(key) = key else {
        return Ok(Outcome::report(all));
    };
    let mut value = Some(&all);
    for part in key.split('.') {
        value = value.and_then(|v| v.get(part));
    }
    let value = value.ok_or_else(|| {
        CliError::usage(format!(
            "no key {key:?}; the keys are {} (and bcachefs.<field>)",
            KEYS.join(", ")
        ))
    })?;
    Ok(Outcome::report(Json::object([(key, value.clone())])).with_text(value.to_text()))
}

/// `stat PATH`: one path in the shape every `fs.<fs> stat` shares, the
/// shared keys first and bcachefs's own under `bcachefs`. The path is
/// resolved as bytes, entering subvolumes, and a symlink is reported, not
/// followed.
fn stat(fs: &Filesystem<Device>, path: &[u8]) -> Result<Outcome, CliError> {
    let n = fs.resolve_bytes(path).map_err(failed)?;
    let i = fs.inode_at(n).map_err(failed)?;
    let raw = fs.inode_raw_at(n).map_err(failed)?;
    let mut fields = vec![(
        "path",
        Json::from(String::from_utf8_lossy(path).into_owned()),
    )];
    if std::str::from_utf8(path).is_err() {
        fields.push((
            "path_hex",
            Json::from(path.iter().map(|b| format!("{b:02x}")).collect::<String>()),
        ));
    }
    fields.extend([
        ("inode", Json::from(n.ino)),
        ("type", Json::from(kind(i.mode))),
        ("mode", Json::from(format!("{:04o}", i.mode & 0o7777))),
        ("uid", Json::from(i.uid)),
        ("gid", Json::from(i.gid)),
        ("nlink", Json::from(i.link_count())),
        ("size", Json::from(i.size)),
        ("allocated_bytes", Json::from(i.sectors.saturating_mul(512))),
    ]);
    // otime is the creation time (the lister's bi_otime).
    for (key, nsec, stored) in [
        ("atime", "atime_nsec", "atime"),
        ("mtime", "mtime_nsec", "mtime"),
        ("ctime", "ctime_nsec", "ctime"),
        ("btime", "btime_nsec", "otime"),
    ] {
        let (s, ns) = epoch_time(fs.superblock(), raw.time(stored).unwrap_or(0));
        fields.push((key, Json::from(s)));
        fields.push((nsec, Json::from(ns)));
    }
    let device = matches!(i.mode & 0o170000, 0o020000 | 0o060000);
    if i.is_symlink() {
        let target = fs.read_at(n).map_err(failed)?;
        fields.push((
            "target",
            Json::from(String::from_utf8_lossy(&target).into_owned()),
        ));
    } else if device {
        // How bi_dev holds the two numbers is not known (docs/clean-room.md,
        // open question 21): no image has held a device node.
        fields.push(("major", Json::Null));
        fields.push(("minor", Json::Null));
    }
    let mut own = vec![
        ("snapshot", Json::from(n.snapshot)),
        ("sectors", Json::from(i.sectors)),
        ("flags", flag_names(i.flags)),
        ("generation", Json::from(raw.field("generation"))),
    ];
    for option in ["data_checksum", "compression", "background_compression"] {
        own.push((option, Json::from(i.option(option))));
    }
    if device {
        own.push(("dev", Json::from(raw.field("dev"))));
    }
    fields.push(("bcachefs", Json::object(own)));
    Ok(Outcome::report(Json::object(fields)))
}

/// bi_flags, the low 20 bits of the inode's flags word: the string hash
/// type and the number of fields are above it.
const BI_FLAGS: u32 = 0xf_ffff;

/// bi_flags bits by the names the reference lister prints for them: only
/// those an image has shown (docs/clean-room.md).
const INODE_FLAGS: &[(u32, &str)] = &[(INODE_HAS_CASE_INSENSITIVE, "has_case_insensitive")];

/// The set bi_flags by name, then every bit without a name as one hex value.
fn flag_names(flags: u32) -> Json {
    let flags = flags & BI_FLAGS;
    let mut names: Vec<Json> = INODE_FLAGS
        .iter()
        .filter(|(bit, _)| flags & bit != 0)
        .map(|(_, name)| Json::from(*name))
        .collect();
    let unknown = INODE_FLAGS.iter().fold(flags, |f, (bit, _)| f & !bit);
    if unknown != 0 {
        names.push(Json::from(format!("0x{unknown:08x}")));
    }
    Json::Arr(names)
}

/// A stored inode time as signed seconds since the epoch and nanoseconds:
/// a count of the superblock's time units (its precision, in nanoseconds)
/// from its time base (`time_base_lo`, nanoseconds since the epoch)
/// (docs/clean-room.md, "Creating a file"). The count is signed, two's
/// complement: a time before the base, as the formatter copies from its
/// source tree, is stored just below 2^64 (docs/clean-room.md, "Inodes and
/// dirents").
fn epoch_time(sb: &Superblock, stored: u64) -> (i64, u64) {
    let ns = i128::from(sb.time_base_lo)
        + i128::from(stored as i64) * i128::from(sb.time_precision.max(1));
    let s = i64::try_from(ns.div_euclid(1_000_000_000)).unwrap_or(i64::MAX);
    (s, ns.rem_euclid(1_000_000_000) as u64)
}

/// The driver supplies bounded positioned reads; core owns the transfer,
/// digest and complete-file publication. Node retains the resolved snapshot.
struct ExportSource<'a> {
    fs: &'a Filesystem<Device>,
    node: Node,
    size: u64,
}

impl transfer::FileSource for ExportSource<'_> {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, CliError> {
        let data = self
            .fs
            .read_range_at(self.node, offset, buf.len())
            .map_err(failed)?;
        buf[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }
}

fn export(
    fs: &Filesystem<Device>,
    request: &transfer::ExportRequest,
    options: transfer::ExportOptions,
) -> Result<Outcome, CliError> {
    let node = fs.resolve_bytes(request.path.as_bytes()).map_err(failed)?;
    let inode = fs.inode_at(node).map_err(failed)?;
    if !inode.is_file() {
        return Err(CliError::failed(format!(
            "{}: not a regular file; directories, symlinks and special files cannot be exported \
             without --recursive",
            request.path.to_string_lossy()
        )));
    }
    transfer::export_with_options(
        request,
        &mut ExportSource {
            fs,
            node,
            size: inode.size,
        },
        options,
    )
}

/// A stored inode time as a `SystemTime`, through [`epoch_time`]: before
/// the epoch as well as after it.
fn system_time(sb: &Superblock, stored: u64) -> Option<std::time::SystemTime> {
    let (s, ns) = epoch_time(sb, stored);
    let seconds = std::time::Duration::from_secs(s.unsigned_abs());
    let base = if s >= 0 {
        std::time::UNIX_EPOCH.checked_add(seconds)?
    } else {
        std::time::UNIX_EPOCH.checked_sub(seconds)?
    };
    base.checked_add(std::time::Duration::from_nanos(ns))
}

/// bcachefs supplies lookups, listings, symlink targets, times, extended
/// attributes and positioned reads. Core owns traversal, the depth bound, safe names,
/// destination merging, publication and the per-item report (#342).
struct TreeExport<'a> {
    fs: &'a Filesystem<Device>,
    image: &'a Path,
    /// The identity core is given for each (inode, snapshot): a snapshot
    /// shares its inode numbers with the live subvolume, so the number
    /// alone would make a snapshot's file a hard link of the subvolume's.
    ids: HashMap<(u64, u32), u64>,
    /// The file `read_file_at` read last, by path, with its size. Core
    /// reads a file in chunks, and resolving its path again for each one
    /// would walk every directory above it every time.
    last: Option<(PathBuf, Node, u64)>,
}

impl TreeExport<'_> {
    fn resolve(&self, path: &Path) -> Result<Node, CliError> {
        self.fs
            .resolve_bytes(path.as_os_str().as_bytes())
            .map_err(|e| at(path, e))
    }

    fn metadata_for(&mut self, n: Node, path: &Path) -> Result<transfer::TreeMetadata, CliError> {
        let fs = self.fs;
        let i = fs.inode_at(n).map_err(|e| at(path, e))?;
        let raw = fs.inode_raw_at(n).map_err(|e| at(path, e))?;
        let next = self.ids.len() as u64;
        let inode = *self.ids.entry((n.ino, n.snapshot)).or_insert(next);
        let time = |name| raw.time(name).and_then(|t| system_time(fs.superblock(), t));
        Ok(transfer::TreeMetadata {
            kind: if i.is_file() {
                transfer::TreeKind::File
            } else if i.is_dir() {
                transfer::TreeKind::Directory
            } else {
                transfer::TreeKind::Other
            },
            size: if i.is_file() { i.size } else { 0 },
            inode,
            accessed: time("atime"),
            modified: time("mtime"),
        })
    }
}

/// A path in an error message: lossy, for people; the bytes are what is
/// resolved. A missing path keeps [`failed`]'s `not_found` kind.
fn at(path: &Path, e: fs_bcachefs::Error) -> CliError {
    match e {
        fs_bcachefs::Error::NotFound(_) => CliError::not_found(format!("{}: {e}", path.display())),
        e => match failed(e) {
            e if e.code == fs_core::cli::output::EXIT_FAILED => {
                CliError::failed(format!("{}: {}", path.display(), e.message))
            }
            e => e,
        },
    }
}

impl transfer::TreeSource for TreeExport<'_> {
    fn image_path(&self) -> Option<&Path> {
        Some(self.image)
    }

    fn symbolic_link(&mut self, path: &Path) -> Result<Option<transfer::SymbolicLink>, CliError> {
        let n = self.resolve(path)?;
        if !self.fs.inode_at(n).map_err(|e| at(path, e))?.is_symlink() {
            return Ok(None);
        }
        let target = self.fs.read_at(n).map_err(|e| at(path, e))?;
        Ok(Some(transfer::SymbolicLink {
            target: PathBuf::from(OsString::from_vec(target)),
            // bcachefs stores no directory bit for a symlink. Never resolve
            // the target to guess one, even when it happens to exist.
            target_is_directory: false,
        }))
    }

    fn metadata(&mut self, path: &Path) -> Result<transfer::TreeMetadata, CliError> {
        let n = self.resolve(path)?;
        self.metadata_for(n, path)
    }

    /// Every attribute the item's snapshot sees, its name spelled with its
    /// namespace prefix. A namespace this driver does not decode fails the
    /// read rather than being left out.
    fn extended_attributes(
        &mut self,
        path: &Path,
    ) -> Result<Vec<transfer::ExtendedAttribute>, CliError> {
        let n = self.resolve(path)?;
        Ok(self
            .fs
            .xattrs_at(n)
            .map_err(|e| at(path, e))?
            .into_iter()
            .map(|x| transfer::ExtendedAttribute {
                name: OsString::from_vec(x.name),
                value: x.value,
            })
            .collect())
    }

    /// bcachefs has no named streams; it stores only extended attributes.
    fn named_streams(&mut self, _path: &Path) -> Result<Vec<transfer::NamedStream>, CliError> {
        Ok(Vec::new())
    }

    fn read_dir(&mut self, path: &Path) -> Result<Vec<transfer::TreeEntry>, CliError> {
        let dir = self.resolve(path)?;
        let mut entries = self.fs.readdir_at(dir).map_err(|e| at(path, e))?;
        entries.retain(|d| d.name != b"." && d.name != b"..");
        entries
            .into_iter()
            .map(|d| {
                let child = path.join(std::ffi::OsStr::from_bytes(&d.name));
                // Type 16 is the reference lister's subvolume dirent, entered
                // as `ls --recursive` enters it (S3, S4).
                let node = match (d.d_type == 16).then_some(d.inum as u32) {
                    Some(id) => self.fs.subvolume(id).map_err(|e| at(&child, e))?,
                    None => Node {
                        ino: d.inum,
                        snapshot: dir.snapshot,
                    },
                };
                Ok(transfer::TreeEntry {
                    metadata: self.metadata_for(node, &child)?,
                    name: OsString::from_vec(d.name),
                })
            })
            .collect()
    }

    fn read_file_at(
        &mut self,
        path: &Path,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, CliError> {
        if !matches!(&self.last, Some((last, _, _)) if last == path) {
            let n = self.resolve(path)?;
            let i = self.fs.inode_at(n).map_err(|e| at(path, e))?;
            if !i.is_file() {
                return Err(CliError::failed(format!(
                    "{}: not a regular file",
                    path.display()
                )));
            }
            self.last = Some((path.to_path_buf(), n, i.size));
        }
        let (_, n, size) = self.last.clone().expect("resolved above");
        if offset >= size {
            return Ok(0);
        }
        let data = self
            .fs
            .read_range_at(n, offset, buf.len())
            .map_err(|e| at(path, e))?;
        buf[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }
}

fn read(
    fs: &Filesystem<Device>,
    path: &str,
    output: Option<&OsString>,
) -> Result<Outcome, CliError> {
    let ino = fs.lookup(path).map_err(failed)?;
    let inode = fs.inode(ino).map_err(failed)?;
    if inode.is_dir() {
        return Err(CliError::failed(format!("{path}: is a directory")));
    }
    if inode.is_symlink() {
        let target = fs.read(ino).map_err(failed)?;
        return Err(CliError::failed(format!(
            "{path}: is a symlink to {}; read the target instead",
            String::from_utf8_lossy(&target)
        )));
    }
    if !inode.is_file() {
        return Err(CliError::failed(format!("{path}: not a regular file")));
    }

    // Keep memory bounded even for sparse files larger than the device.
    const CHUNK: usize = 1 << 20;
    let copy = |sink: &mut dyn Write| -> Result<(), CliError> {
        let mut offset = 0u64;
        while offset < inode.size {
            let want = (inode.size - offset).min(CHUNK as u64) as usize;
            let data = fs.read_range(ino, offset, want).map_err(failed)?;
            if data.len() != want {
                return Err(CliError::failed(format!(
                    "{path}: short read at byte {offset} of {}",
                    inode.size
                )));
            }
            sink.write_all(&data)
                .map_err(|e| CliError::failed(format!("write: {e}")))?;
            offset += data.len() as u64;
        }
        sink.flush()
            .map_err(|e| CliError::failed(format!("write: {e}")))
    };
    match output {
        None => copy(&mut std::io::stdout().lock())?,
        Some(file) => {
            let dest = std::path::Path::new(file);
            let mut partial = dest.as_os_str().to_owned();
            partial.push(".partial");
            let partial = std::path::PathBuf::from(partial);
            // Do not truncate a partial left by another extraction, or
            // follow a host symlink at that path.
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&partial)
                .map_err(|e| CliError::failed(format!("create {}: {e}", partial.display())))?;
            let result = copy(&mut f);
            drop(f);
            let result = result.and_then(|()| {
                std::fs::rename(&partial, dest)
                    .map_err(|e| CliError::failed(format!("rename to {}: {e}", dest.display())))
            });
            if result.is_err() {
                let _ = std::fs::remove_file(&partial);
            }
            result?;
        }
    }
    Ok(Outcome::done())
}

/// The type of an inode by its mode, in the words the manifests use.
fn kind(mode: u32) -> &'static str {
    match mode & 0o170000 {
        0o040000 => "dir",
        0o100000 => "file",
        0o120000 => "symlink",
        0o020000 => "char",
        0o060000 => "block",
        0o010000 => "fifo",
        0o140000 => "socket",
        _ => "unknown",
    }
}

fn ls_entry(name: &[u8], kind: &str, ino: u64, size: u64) -> Json {
    Json::object([
        (
            "name",
            Json::Str(String::from_utf8_lossy(name).into_owned()),
        ),
        ("type", Json::Str(kind.into())),
        ("ino", Json::UInt(ino)),
        ("size", Json::UInt(size)),
    ])
}

fn recursive_entry(
    path: &[u8],
    depth: u64,
    n: Node,
    i: &Inode,
    subvol: Option<u32>,
    revisited: bool,
) -> Json {
    let d = Dirent {
        dir: 0,
        name: Vec::new(),
        inum: n.ino,
        d_type: (i.mode >> 12) as u8,
    };
    let name = path.rsplit(|b| *b == b'/').next().unwrap_or(path);
    let Json::Obj(mut fields) = ls_entry(name, d.type_name(), n.ino, i.size) else {
        unreachable!()
    };
    fields.push((
        "path".into(),
        Json::Str(String::from_utf8_lossy(path).into_owned()),
    ));
    if std::str::from_utf8(path).is_err() {
        fields.push((
            "path_hex".into(),
            Json::Str(path.iter().map(|b| format!("{b:02x}")).collect()),
        ));
    }
    fields.push(("depth".into(), Json::UInt(depth)));
    if let Some(id) = subvol {
        fields.push(("subvolume".into(), Json::UInt(id.into())));
        fields.push(("snapshot".into(), Json::UInt(n.snapshot.into())));
    }
    if revisited {
        fields.push(("revisited".into(), Json::Bool(true)));
    }
    Json::Obj(fields)
}

/// Heap stack: one directory's remaining entries per level. Identity includes
/// the snapshot, since its inode numbers are shared with the live subvolume.
fn ls_recursive(
    fs: &Filesystem<Device>,
    sub: &ArgMatches,
    format: Format,
) -> Result<Outcome, CliError> {
    let path = sub
        .get_one::<OsString>("path")
        .expect("defaulted")
        .as_bytes();
    let at = |path: &[u8], e: fs_bcachefs::Error| {
        let message = format!("{}: {e}", String::from_utf8_lossy(path));
        match e {
            fs_bcachefs::Error::NotFound(_) => CliError::not_found(message),
            _ => CliError::failed(message),
        }
    };
    let root = fs.resolve_bytes(path).map_err(|e| at(path, e))?;
    let inode = fs.inode_at(root).map_err(|e| at(path, e))?;
    let max_depth = sub.get_one::<u64>("max-depth").copied().unwrap_or(64);
    let limit = sub.get_one::<u64>("limit").copied().unwrap_or(100_000);
    let mut start = Vec::new();
    for part in path.split(|b| *b == b'/').filter(|p| !p.is_empty()) {
        start.push(b'/');
        start.extend_from_slice(part);
    }
    if start.is_empty() {
        start.push(b'/');
    }
    let children = |n, path: &[u8]| {
        let mut entries = fs.readdir_at(n).map_err(|e| at(path, e))?;
        entries.retain(|d| d.name != b"." && d.name != b"..");
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok::<_, CliError>(entries.into_iter())
    };
    let reached = |bound, value, path: &[u8]| {
        CliError::failed(format!(
            "{bound} {value} reached at {}",
            String::from_utf8_lossy(path)
        ))
    };
    let mut rows = Vec::new();
    let walked = (|| {
        if !inode.is_dir() {
            if limit == 0 {
                return Err(reached("--limit", limit, &start));
            }
            rows.push(recursive_entry(&start, 0, root, &inode, None, false));
            return Ok(());
        }
        let first = children(root, &start)?;
        let mut visited = HashSet::from([(root.ino, root.snapshot)]);
        let mut stack = vec![(root, start, 0u64, first)];
        while let Some((parent, prefix, parent_depth, entries)) = stack.last_mut() {
            let Some(d) = entries.next() else {
                stack.pop();
                continue;
            };
            let mut path = prefix.clone();
            if !path.ends_with(b"/") {
                path.push(b'/');
            }
            path.extend_from_slice(&d.name);
            let depth = *parent_depth + 1;
            if rows.len() as u64 >= limit {
                return Err(reached("--limit", limit, &path));
            }
            if depth > max_depth {
                return Err(reached("--max-depth", max_depth, &path));
            }
            // Type 16 is the reference lister's subvolume dirent (S3, S4).
            let subvol = (d.d_type == 16).then_some(d.inum as u32);
            let node = match subvol {
                Some(id) => fs.subvolume(id).map_err(|e| at(&path, e))?,
                None => Node {
                    ino: d.inum,
                    snapshot: parent.snapshot,
                },
            };
            let i = fs.inode_at(node).map_err(|e| at(&path, e))?;
            let revisited = i.is_dir() && !visited.insert((node.ino, node.snapshot));
            rows.push(recursive_entry(&path, depth, node, &i, subvol, revisited));
            if i.is_dir() && !revisited {
                let next = children(node, &path)?;
                stack.push((node, path, depth, next));
            }
        }
        Ok(())
    })();
    let text = rows
        .iter()
        .map(|r| {
            let field = |key| r.get(key).map(Json::to_text).unwrap_or_default();
            format!(
                "{} {} {} {}{}",
                field("type"),
                field("ino"),
                field("size"),
                field("path"),
                if r.get("revisited") == Some(&Json::Bool(true)) {
                    " [revisited]"
                } else {
                    ""
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let report = Outcome::report(Json::Arr(rows)).with_text(text);
    if let Err(error) = walked {
        let partial = fs_core::cli::output::render(TOOL.name, format, Ok(report));
        let mut out = std::io::stdout().lock();
        out.write_all(partial.stdout.as_bytes())
            .and_then(|()| out.flush())
            .map_err(|e| CliError::failed(format!("write: {e}")))?;
        return Err(error);
    }
    Ok(report)
}

/// A library error as the CLI contract has it. `NotFound` is the library's
/// "the path or key does not exist" (the C ABI's ENOENT), so it is the
/// contract's `not_found` kind, still exit 1, which a script matches
/// whatever the message says.
fn failed(e: fs_bcachefs::Error) -> CliError {
    match e {
        fs_bcachefs::Error::Unsupported(m) => CliError::not_implemented(m),
        fs_bcachefs::Error::NotFound(_) => CliError::not_found(e.to_string()),
        e => CliError::failed(e.to_string()),
    }
}

/// Opening looks nothing up by path, so a structure missing there is
/// damage, an ordinary failure, not a missing path.
fn open(dev: Device) -> Result<Filesystem<Device>, CliError> {
    Filesystem::open(dev).map_err(|e| match e {
        fs_bcachefs::Error::NotFound(_) => CliError::failed(e.to_string()),
        e => failed(e),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `kind` the JSON error carries, as compact JSON.
    fn json_kind(e: &CliError) -> Option<String> {
        e.to_json().get("kind").map(Json::to_compact)
    }

    #[test]
    fn a_missing_path_is_the_not_found_kind() {
        // What lookup and resolve_bytes answer for a name that is not
        // there, or for one under a parent that is not there.
        let missing = || fs_bcachefs::Error::NotFound("/missing/f".into());
        for e in [failed(missing()), at(Path::new("/missing/f"), missing())] {
            let json = e.to_json().to_compact();
            assert_eq!(e.code, fs_core::cli::output::EXIT_FAILED, "{json}");
            assert_eq!(json_kind(&e).as_deref(), Some("\"not_found\""), "{json}");
        }
    }

    #[test]
    fn damage_and_limits_carry_no_kind() {
        for e in [
            failed(fs_bcachefs::Error::Corrupt("btree node".into())),
            failed(fs_bcachefs::Error::Unsupported("encryption".into())),
            at(Path::new("/f"), fs_bcachefs::Error::Io("short read".into())),
        ] {
            assert_eq!(json_kind(&e), None, "{}", e.to_json().to_compact());
        }
    }
}
