//! `fsck.bcachefs`: check a bcachefs filesystem without changing it.
//!
//! What it checks is this crate's checker (`fs_bcachefs::check`), a subset
//! of what the reference checker checks, and it says so: a filesystem this
//! calls clean is one on which none of THESE invariants is broken.
//!
//! It never writes. `-n` is accepted because scripts pass it; `-y` and `-p`
//! (repair) are refused, because there is no repair.
//!
//! EXIT STATUS IS fsck(8)'s: 0 clean, 4 errors left uncorrected, 8 an
//! operational error (the target could not be opened, or is not bcachefs,
//! or cannot be checked), 16 a wrong command line.

use std::ffi::OsString;

use clap::{value_parser, Arg, ArgAction, ArgMatches, Command as Cmd};
use fs_core::cli::{CliError, Json, Outcome, Tool};
use fs_core::FileDevice;

/// fsck(8): filesystem errors left uncorrected.
pub const UNCORRECTED: u8 = 4;
/// fsck(8): operational error.
pub const OPERATIONAL: u8 = 8;
/// fsck(8): usage or syntax error.
pub const USAGE: u8 = 16;

pub const TOOL: Tool = Tool {
    name: "fsck.bcachefs",
    verb: "fsck",
    section: 8,
    usage_exit: USAGE,
    about: "Check a bcachefs filesystem without changing it",
    command,
    run,
};

fn command() -> Cmd {
    Cmd::new("fsck.bcachefs")
        .about("Check a bcachefs filesystem without changing it")
        .long_about(
            "Check a bcachefs image or device and report what is wrong with it. Nothing is \
             written, and nothing is repaired.\n\n\
             Checked: the superblock; every node of every btree (magic, sequence, checksums, \
             key order); a size, snapshot, position or value length a key's btree does not \
             allow; every directory entry against the inode it names; every inode's link count \
             and back-reference; every extent against its inode; every xattr against its inode; \
             snapshots and subvolumes; reflink ranges and refcounts; and every data checksum. A \
             filesystem not shut down cleanly is checked through a replay of its journal, in \
             memory.\n\n\
             Also checked, on a filesystem shut down cleanly: each bucket's generation in \
             bucket_gens against its alloc key; each data extent's backpointer; each bucket's \
             data type; and, on one device, the freespace btree, the fragmentation lru and the \
             need_discard btree against the buckets, and the device's accounting by data type \
             against its alloc keys.\n\n\
             Not checked: the rest of what the reference checker checks of allocation, \
             accounting, backpointers, freespace and lru.\n\n\
             Exit status is fsck(8)'s: 0 clean, 4 errors found (and left), 8 the target could \
             not be checked, 16 a wrong command line.",
        )
        .arg(
            Arg::new("target")
                .value_name("TARGET")
                .help("Block device or image file to check")
                .required(true)
                .value_parser(value_parser!(OsString)),
        )
        .arg(
            Arg::new("no-change")
                .short('n')
                .help("Check only (the default, and the only mode)")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("repair")
                .short('y')
                .help("Refused: this checker does not repair")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("preen")
                .short('p')
                .help("Refused: this checker does not repair")
                .action(ArgAction::SetTrue),
        )
        .args(fs_core::cli::format_args())
        .after_help(
            "Examples:\n  \
             fsck.bcachefs disk.img           the report, as JSON\n  \
             fsck.bcachefs --text disk.img    the findings, one per line",
        )
}

fn run(m: &ArgMatches) -> Result<Outcome, CliError> {
    if m.get_flag("repair") || m.get_flag("preen") {
        return Err(
            CliError::usage("this checker does not repair: run it without -y or -p")
                .with_code(USAGE),
        );
    }
    let target = m.get_one::<OsString>("target").expect("required");
    let dev = FileDevice::open(target)
        .map_err(|e| CliError::failed(e.to_string()).with_code(OPERATIONAL))?;
    let r = fs_bcachefs::check::check(&dev)
        .map_err(|e| CliError::failed(e.to_string()).with_code(OPERATIONAL))?;
    let problems: Vec<Json> = r
        .problems
        .iter()
        .map(|p| {
            Json::object([
                ("kind", Json::Str(p.kind.into())),
                ("detail", Json::Str(p.detail.clone())),
            ])
        })
        .collect();
    let text: String = if r.clean() {
        format!(
            "clean: {} inodes, {} directory entries, {} extents{}",
            r.inodes,
            r.dirents,
            r.extents,
            if r.replayed {
                " (through a replay of the journal)"
            } else {
                ""
            }
        )
    } else {
        r.problems
            .iter()
            .map(|p| format!("{}: {}", p.kind, p.detail))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let code = if r.clean() { 0 } else { UNCORRECTED };
    Ok(Outcome::report(Json::object([
        ("clean", Json::Bool(r.clean())),
        ("replayed", Json::Bool(r.replayed)),
        ("inodes", Json::UInt(r.inodes)),
        ("dirents", Json::UInt(r.dirents)),
        ("extents", Json::UInt(r.extents)),
        ("problems", Json::Arr(problems)),
        ("exit", Json::UInt(u64::from(code))),
    ]))
    .with_text(text)
    .with_code(code))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Checking section of the features page.
    fn checking_section() -> &'static str {
        let page = include_str!("../../docs/features.md");
        let (_, rest) = page
            .split_once("\n## Checking\n")
            .expect("the features page has a Checking section");
        rest.split_once("\n## ")
            .map_or(rest, |(section, _)| section)
    }

    /// `--help` as it prints, with its line wrapping taken out.
    fn long_help() -> String {
        let help = command().render_long_help().to_string();
        help.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Each entry is a thing the help says is checked, the words the features
    /// page uses for it, and a problem the checker reports for it.
    #[test]
    fn the_help_names_what_the_checker_and_the_features_page_name() {
        let checker = include_str!("../check.rs");
        let help = long_help();
        let page = checking_section();
        for (in_help, on_page, kind) in [
            ("link count", "link counts", "\"link_count\""),
            ("xattr", "xattr", "\"hash_table_key_wrong_offset\""),
            ("reflink", "reflink", "\"reflink_refcount\""),
            (
                "bucket_gens",
                "bucket_gens",
                "\"incorrect_gen_in_bucket_gens\"",
            ),
            (
                "backpointer",
                "backpointer",
                "\"backpointer_to_missing_ptr\"",
            ),
            ("data type", "data type", "\"alloc_key_data_type_wrong\""),
            (
                "fragmentation lru",
                "fragmentation lru",
                "\"missing_fragmentation_lru_entry\"",
            ),
            (
                "need_discard",
                "need_discard",
                "\"missing_need_discard_entry\"",
            ),
            ("accounting", "accounting", "\"accounting_mismatch\""),
        ] {
            assert!(help.contains(in_help), "--help omits {in_help}:\n{help}");
            assert!(page.contains(on_page), "the page omits {on_page}:\n{page}");
            assert!(checker.contains(kind), "the checker has no {kind}");
        }
    }

    /// The help used to deny the allocation checks wholesale. What it says is
    /// missing is the remainder, and the condition they are made under is
    /// stated.
    #[test]
    fn the_help_says_what_is_not_checked_and_when_allocation_is() {
        let help = long_help();
        assert!(
            !help.contains("backpointers are not checked"),
            "--help denies checks the checker makes:\n{help}"
        );
        assert!(
            help.contains("Not checked: the rest of"),
            "--help does not say what is left out:\n{help}"
        );
        assert!(
            help.contains("shut down cleanly: "),
            "--help does not say when allocation is judged:\n{help}"
        );
    }
}
