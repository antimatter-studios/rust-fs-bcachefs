//! `fs.bcachefs`: inspect a bcachefs image, and (experimental, src/cli/
//! fs_write.rs) change it.

use std::ffi::OsString;
use std::io::Write;

use clap::{value_parser, Arg, ArgMatches, Command as Cmd};

use fs_bcachefs::superblock::{format_uuid, Superblock};
use fs_bcachefs::Filesystem;
use fs_core::cli::{CliError, Json, Outcome, Tool};
use fs_core::FileDevice;

pub const TOOL: Tool = Tool {
    name: "fs.bcachefs",
    verb: "fs",
    section: 1,
    usage_exit: fs_core::cli::output::EXIT_USAGE,
    about: "Inspect a bcachefs image or device read-only, without mounting it",
    command,
    run,
};

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
        .args(fs_core::cli::format_args().map(|a| a.global(true)))
        .subcommand_required(true)
        .subcommand(Cmd::new("info").about("The superblock: identity, version, geometry"))
        .subcommand(
            Cmd::new("ls")
                .about("List a directory: name, type, inode, size")
                .arg(Arg::new("path").value_name("PATH").default_value("/")),
        )
        .subcommand(
            Cmd::new("stat")
                .about("One path's inode: number, type, mode, owner, links, size, times")
                .arg(Arg::new("path").value_name("PATH").required(true)),
        )
        .subcommand(
            Cmd::new("tree")
                .about("Every path under a directory, recursively: path, type, inode, size")
                .arg(Arg::new("path").value_name("PATH").default_value("/")),
        )
        .subcommand(
            Cmd::new("cat")
                .about("Write a file's bytes (or a symlink's target) to stdout")
                .arg(Arg::new("path").value_name("PATH").required(true)),
        )
}

fn run(m: &ArgMatches) -> Result<Outcome, CliError> {
    let target = m.get_one::<OsString>("target").expect("required");
    if let Some((verb, sub)) = m.subcommand() {
        if let Some(r) = super::fs_write::run(target, verb, sub) {
            return r;
        }
    }
    let dev = FileDevice::open(target).map_err(|e| CliError::failed(e.to_string()))?;
    match m.subcommand() {
        Some(("info", _)) => {
            let sb = Superblock::read(&dev).map_err(failed)?;
            Ok(Outcome::report(Json::object([
                ("fs", Json::Str("bcachefs".into())),
                ("label", Json::Str(sb.label_str())),
                ("uuid", Json::Str(format_uuid(&sb.user_uuid))),
                (
                    "version",
                    Json::Str(format!("{}.{}", sb.version.major(), sb.version.minor())),
                ),
                ("block_size", Json::UInt(sb.block_size as u64 * 512)),
                (
                    "btree_node_size",
                    Json::UInt(sb.btree_node_size() as u64 * 512),
                ),
                ("devices", Json::UInt(sb.nr_devices as u64)),
                ("encrypted", Json::Bool(sb.is_encrypted())),
                ("clean", Json::Bool(sb.is_clean())),
            ])))
        }
        Some(("ls", sub)) => {
            let fs = open(dev)?;
            let path = sub.get_one::<String>("path").expect("defaulted");
            let ino = fs.lookup(path).map_err(failed)?;
            let mut rows = Vec::new();
            for d in fs.readdir(ino).map_err(failed)? {
                let size = fs.inode(d.inum).map(|i| i.size).unwrap_or(0);
                rows.push(Json::object([
                    (
                        "name",
                        Json::Str(String::from_utf8_lossy(&d.name).into_owned()),
                    ),
                    ("type", Json::Str(d.type_name().into())),
                    ("ino", Json::UInt(d.inum)),
                    ("size", Json::UInt(size)),
                ]));
            }
            Ok(Outcome::report(Json::Arr(rows)))
        }
        Some(("stat", sub)) => {
            let fs = open(dev)?;
            let path = sub.get_one::<String>("path").expect("required");
            let ino = fs.lookup(path).map_err(failed)?;
            let i = fs.inode(ino).map_err(failed)?;
            let base = fs.superblock().time_base_lo;
            Ok(Outcome::report(Json::object([
                ("path", Json::Str(path.clone())),
                ("ino", Json::UInt(ino)),
                ("type", Json::Str(kind(i.mode).into())),
                ("mode", Json::Str(format!("{:04o}", i.mode & 0o7777))),
                ("uid", Json::UInt(i.uid.into())),
                ("gid", Json::UInt(i.gid.into())),
                ("nlink", Json::UInt(i.link_count().into())),
                ("size", Json::UInt(i.size)),
                ("sectors", Json::UInt(i.sectors)),
                ("atime_ns", Json::UInt(base.saturating_add(i.atime))),
                ("mtime_ns", Json::UInt(base.saturating_add(i.mtime))),
                ("ctime_ns", Json::UInt(base.saturating_add(i.ctime))),
            ])))
        }
        Some(("tree", sub)) => {
            let fs = open(dev)?;
            let path = sub.get_one::<String>("path").expect("defaulted");
            let root = fs.lookup(path).map_err(failed)?;
            let mut rows = Vec::new();
            let prefix = path.trim_end_matches('/').to_string();
            walk(&fs, root, &prefix, &mut rows, 0)?;
            Ok(Outcome::report(Json::Arr(rows)))
        }
        Some(("cat", sub)) => {
            let fs = open(dev)?;
            let path = sub.get_one::<String>("path").expect("required");
            let data = fs
                .lookup(path)
                .and_then(|ino| fs.read(ino))
                .map_err(failed)?;
            std::io::stdout()
                .write_all(&data)
                .map_err(|e| CliError::failed(e.to_string()))?;
            Ok(Outcome::done())
        }
        _ => Err(CliError::usage("unknown subcommand")),
    }
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

/// Deeper than any real tree; a directory cycle in a corrupt image stops here.
const MAX_TREE_DEPTH: usize = 256;

fn walk(
    fs: &Filesystem<FileDevice>,
    dir: u64,
    prefix: &str,
    rows: &mut Vec<Json>,
    depth: usize,
) -> Result<(), CliError> {
    if depth > MAX_TREE_DEPTH {
        return Err(CliError::failed("directories nest deeper than 256 levels"));
    }
    let mut entries: Vec<_> = fs.readdir(dir).map_err(failed)?.to_vec();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    for d in entries {
        let path = format!("{prefix}/{}", String::from_utf8_lossy(&d.name));
        let i = fs.inode(d.inum).map_err(failed)?;
        rows.push(Json::object([
            ("path", Json::Str(path.clone())),
            ("type", Json::Str(kind(i.mode).into())),
            ("ino", Json::UInt(d.inum)),
            ("size", Json::UInt(i.size)),
        ]));
        if i.is_dir() {
            walk(fs, d.inum, &path, rows, depth + 1)?;
        }
    }
    Ok(())
}

fn failed(e: fs_bcachefs::Error) -> CliError {
    match e {
        fs_bcachefs::Error::Unsupported(m) => CliError::not_implemented(m),
        e => CliError::failed(e.to_string()),
    }
}

fn open(dev: FileDevice) -> Result<Filesystem<FileDevice>, CliError> {
    Filesystem::open(dev).map_err(failed)
}
