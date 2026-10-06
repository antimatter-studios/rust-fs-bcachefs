//! `fs.bcachefs`: inspect a bcachefs image read-only.

use std::ffi::OsString;

use clap::{value_parser, Arg, ArgMatches, Command as Cmd};

use fs_bcachefs::superblock::{format_uuid, Superblock};
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
}

fn run(m: &ArgMatches) -> Result<Outcome, CliError> {
    let target = m.get_one::<OsString>("target").expect("required");
    let dev = FileDevice::open(target).map_err(|e| CliError::failed(e.to_string()))?;
    match m.subcommand() {
        Some(("info", _)) => {
            let sb = Superblock::read(&dev).map_err(|e| CliError::failed(e.to_string()))?;
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
        _ => Err(CliError::usage("unknown subcommand")),
    }
}
