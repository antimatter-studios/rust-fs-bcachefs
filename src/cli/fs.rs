//! `fs.bcachefs`: inspect a bcachefs image read-only.

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
            Cmd::new("cat")
                .about("Write a file's bytes (or a symlink's target) to stdout")
                .arg(Arg::new("path").value_name("PATH").required(true)),
        )
}

fn run(m: &ArgMatches) -> Result<Outcome, CliError> {
    let target = m.get_one::<OsString>("target").expect("required");
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

fn failed(e: fs_bcachefs::Error) -> CliError {
    match e {
        fs_bcachefs::Error::Unsupported(m) => CliError::not_implemented(m),
        e => CliError::failed(e.to_string()),
    }
}

fn open(dev: FileDevice) -> Result<Filesystem<FileDevice>, CliError> {
    Filesystem::open(dev).map_err(failed)
}
