//! `fs.bcachefs`'s write verbs: put, mkdir, rm, rmdir, mv, ln, chmod, chown,
//! setfattr, rmfattr. EXPERIMENTAL: each is one transaction of
//! `fs_bcachefs::write::Writer`, which the tests hold to the reference
//! checker and the reference mount; `--journal` commits it through the
//! journal instead of in place.

use std::ffi::OsString;
use std::io::Read;

use clap::{value_parser, Arg, ArgAction, ArgMatches, Command as Cmd};
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::cli::{CliError, Json, Outcome};
use fs_core::FileDevice;

const VERBS: &[&str] = &[
    "put", "mkdir", "rm", "rmdir", "mv", "ln", "chmod", "chown", "setfattr", "rmfattr",
];

fn journal() -> Arg {
    Arg::new("journal")
        .long("journal")
        .help("Commit through the journal: the filesystem is replayed when next opened")
        .action(ArgAction::SetTrue)
}

fn path(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name).value_name("PATH").help(help).required(true)
}

/// The write verbs, added to `fs.bcachefs`'s command.
pub fn subcommands(cmd: Cmd) -> Cmd {
    cmd.subcommand(
        Cmd::new("put")
            .about("(experimental) Create or replace a file with stdin, or --from FILE")
            .arg(path("path", "The file to write"))
            .arg(
                Arg::new("from")
                    .long("from")
                    .value_name("FILE")
                    .value_parser(value_parser!(OsString))
                    .help("Read the contents from FILE instead of stdin"),
            )
            .arg(
                Arg::new("mode")
                    .long("mode")
                    .value_name("OCTAL")
                    .default_value("644"),
            )
            .arg(journal()),
    )
    .subcommand(
        Cmd::new("mkdir")
            .about("(experimental) Create a directory")
            .arg(path("path", "The directory to create"))
            .arg(
                Arg::new("mode")
                    .long("mode")
                    .value_name("OCTAL")
                    .default_value("755"),
            )
            .arg(journal()),
    )
    .subcommand(
        Cmd::new("rm")
            .about("(experimental) Remove a file or symlink")
            .arg(path("path", "The name to remove"))
            .arg(journal()),
    )
    .subcommand(
        Cmd::new("rmdir")
            .about("(experimental) Remove an empty directory")
            .arg(path("path", "The directory to remove"))
            .arg(journal()),
    )
    .subcommand(
        Cmd::new("mv")
            .about("(experimental) Rename: FROM to TO, which must not exist")
            .arg(path("from", "The name to move"))
            .arg(path("to", "Its new name"))
            .arg(journal()),
    )
    .subcommand(
        Cmd::new("ln")
            .about("(experimental) Make a hard link, or with -s a symlink to TARGET")
            .arg(
                Arg::new("symbolic")
                    .short('s')
                    .action(ArgAction::SetTrue)
                    .help("Make a symlink whose target is TARGET, as given"),
            )
            .arg(Arg::new("target").value_name("TARGET").required(true))
            .arg(path("path", "The new name"))
            .arg(journal()),
    )
    .subcommand(
        Cmd::new("chmod")
            .about("(experimental) Set permissions")
            .arg(Arg::new("mode").value_name("OCTAL").required(true))
            .arg(path("path", "The path to change"))
            .arg(journal()),
    )
    .subcommand(
        Cmd::new("chown")
            .about("(experimental) Set owner and group: UID:GID, UID: or :GID")
            .arg(Arg::new("owner").value_name("UID:GID").required(true))
            .arg(path("path", "The path to change"))
            .arg(journal()),
    )
    .subcommand(
        Cmd::new("setfattr")
            .about("(experimental) Set an extended attribute (user.* or trusted.*)")
            .arg(Arg::new("name").value_name("NAME").required(true))
            .arg(Arg::new("value").value_name("VALUE").required(true))
            .arg(path("path", "The path to change"))
            .arg(journal()),
    )
    .subcommand(
        Cmd::new("rmfattr")
            .about("(experimental) Remove an extended attribute")
            .arg(Arg::new("name").value_name("NAME").required(true))
            .arg(path("path", "The path to change"))
            .arg(journal()),
    )
}

fn failed(e: fs_bcachefs::Error) -> CliError {
    match e {
        fs_bcachefs::Error::Unsupported(m) => CliError::not_implemented(m),
        e => CliError::failed(e.to_string()),
    }
}

/// A path's parent directory, by inode, and its last component.
fn split(target: &OsString, p: &str) -> Result<(u64, Vec<u8>), CliError> {
    let trimmed = p.trim_end_matches('/');
    let (dir, name) = match trimmed.rfind('/') {
        Some(i) => (&trimmed[..i], &trimmed[i + 1..]),
        None => ("", trimmed),
    };
    if name.is_empty() {
        return Err(CliError::usage(format!("{p:?} names no entry")));
    }
    let fs =
        Filesystem::open(FileDevice::open(target).map_err(|e| CliError::failed(e.to_string()))?)
            .map_err(failed)?;
    let dir = fs
        .lookup(if dir.is_empty() { "/" } else { dir })
        .map_err(failed)?;
    Ok((dir, name.as_bytes().to_vec()))
}

fn lookup(target: &OsString, p: &str) -> Result<u64, CliError> {
    let fs =
        Filesystem::open(FileDevice::open(target).map_err(|e| CliError::failed(e.to_string()))?)
            .map_err(failed)?;
    fs.lookup(p).map_err(failed)
}

fn octal(s: &str) -> Result<u32, CliError> {
    u32::from_str_radix(s, 8).map_err(|_| CliError::usage(format!("{s:?} is not an octal mode")))
}

/// Run a write verb, or `None` when `verb` is not one.
pub fn run(target: &OsString, verb: &str, m: &ArgMatches) -> Option<Result<Outcome, CliError>> {
    if !VERBS.contains(&verb) {
        return None;
    }
    Some(run_verb(target, verb, m))
}

fn run_verb(target: &OsString, verb: &str, m: &ArgMatches) -> Result<Outcome, CliError> {
    let s = |k: &str| m.get_one::<String>(k).cloned().unwrap_or_default();
    let open = || -> Result<Writer<FileDevice>, CliError> {
        let dev = FileDevice::open_rw(target).map_err(|e| CliError::failed(e.to_string()))?;
        if m.get_flag("journal") {
            // Continues a journal an earlier --journal left for replay.
            Writer::open_journalled(dev).map_err(failed)
        } else {
            Writer::open(dev).map_err(failed)
        }
    };
    let mut report = vec![("verb", Json::Str(verb.into()))];
    match verb {
        "put" => {
            let mut data = Vec::new();
            match m.get_one::<OsString>("from") {
                Some(f) => {
                    data = std::fs::read(f).map_err(|e| CliError::failed(format!("{f:?}: {e}")))?;
                }
                None => {
                    std::io::stdin()
                        .read_to_end(&mut data)
                        .map_err(|e| CliError::failed(e.to_string()))?;
                }
            }
            let p = s("path");
            let mode = octal(&s("mode"))?;
            let existing = lookup(target, &p).ok();
            let (dir, name) = split(target, &p)?;
            let mut w = open()?;
            let ino = match existing {
                Some(ino) => {
                    w.write_file(ino, &data).map_err(failed)?;
                    ino
                }
                None => w.create_file(dir, &name, &data, mode).map_err(failed)?,
            };
            report.push(("ino", Json::UInt(ino)));
            report.push(("size", Json::UInt(data.len() as u64)));
        }
        "mkdir" => {
            let (dir, name) = split(target, &s("path"))?;
            let mode = octal(&s("mode"))?;
            let ino = open()?.mkdir(dir, &name, mode).map_err(failed)?;
            report.push(("ino", Json::UInt(ino)));
        }
        "rm" => {
            let (dir, name) = split(target, &s("path"))?;
            open()?.unlink(dir, &name).map_err(failed)?;
        }
        "rmdir" => {
            let (dir, name) = split(target, &s("path"))?;
            open()?.rmdir(dir, &name).map_err(failed)?;
        }
        "mv" => {
            let (from_dir, from) = split(target, &s("from"))?;
            let (to_dir, to) = split(target, &s("to"))?;
            open()?
                .rename(from_dir, &from, to_dir, &to)
                .map_err(failed)?;
        }
        "ln" => {
            let (dir, name) = split(target, &s("path"))?;
            let t = s("target");
            if m.get_flag("symbolic") {
                let ino = open()?.symlink(dir, &name, t.as_bytes()).map_err(failed)?;
                report.push(("ino", Json::UInt(ino)));
            } else {
                let ino = lookup(target, &t)?;
                open()?.link(ino, dir, &name).map_err(failed)?;
                report.push(("ino", Json::UInt(ino)));
            }
        }
        "chmod" => {
            let ino = lookup(target, &s("path"))?;
            open()?
                .set_attributes(ino, Some(octal(&s("mode"))?), None, None)
                .map_err(failed)?;
        }
        "chown" => {
            let ino = lookup(target, &s("path"))?;
            let owner = s("owner");
            let (u, g) = owner
                .split_once(':')
                .ok_or_else(|| CliError::usage("owner is UID:GID, UID: or :GID"))?;
            let num = |v: &str| -> Result<Option<u32>, CliError> {
                if v.is_empty() {
                    Ok(None)
                } else {
                    v.parse()
                        .map(Some)
                        .map_err(|_| CliError::usage(format!("{v:?} is not a number")))
                }
            };
            open()?
                .set_attributes(ino, None, num(u)?, num(g)?)
                .map_err(failed)?;
        }
        "setfattr" => {
            let ino = lookup(target, &s("path"))?;
            open()?
                .set_xattr(ino, s("name").as_bytes(), s("value").as_bytes())
                .map_err(failed)?;
        }
        "rmfattr" => {
            let ino = lookup(target, &s("path"))?;
            open()?
                .remove_xattr(ino, s("name").as_bytes())
                .map_err(failed)?;
        }
        _ => unreachable!("checked against VERBS"),
    }
    report.push(("journal", Json::Bool(m.get_flag("journal"))));
    Ok(Outcome::report(Json::object(report)))
}
