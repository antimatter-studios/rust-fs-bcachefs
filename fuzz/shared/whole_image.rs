// Whole-image fuzzing (#152), shared by both halves the way helpers.rs is,
// and included after it (it uses helpers.rs's re-stamps and readers):
// `tests/oracle_fuzz_whole_image.rs` is the gate, on the stable toolchain
// in every pull request that has the fixtures, and
// `fuzz/fuzz_targets/whole_image.rs` the explorer.
//
// The decoder targets each drive one structure. A hostile image reaches
// the whole stack: `Filesystem::open`, the btree cursor and the node
// cache, journal replay, snapshot chains, reflink indirection, the checker
// and the writer. Most of the arithmetic on disk values lives there. A case
// here is one fixture with one change laid over it, and then that whole
// workload.
//
// - An overlay device. The fixture is read once and shared; a case's change,
//   and whatever the writer writes, are pages laid over it. A case costs a
//   page or two, not a copy of the image.
// - Only what is read is mutated. A clean pass logs every read, and the
//   btree nodes, journal entries and superblock among them are the targets:
//   a byte flipped in free space finds nothing.
// - Mutate, then re-stamp the checksum (three times in four; the fourth is
//   a torn write), for the reason helpers.rs gives. Or change one field of
//   one real key and commit it through the writer's raw insert, which packs
//   it into a node with valid checksums, so the case reaches the semantic
//   code rather than the checksum check.
// - The workload: open, a bounded walk (resolve, inode, xattrs, readdir and
//   reads), the checker, and on an image the checker passes, a few writes
//   and both again.

/// The overlay's page: a write or a patch copies the base's page once.
const WI_PAGE: u64 = 4096;

/// A fixture with pages laid over it. Reads see the pages where there are
/// any and the fixture elsewhere; writes land in pages.
pub struct Overlay {
    base: std::sync::Arc<Vec<u8>>,
    pages: std::sync::Mutex<std::collections::BTreeMap<u64, Vec<u8>>>,
    /// Every read, `(offset, length)`, while a clean pass logs them.
    log: Option<std::sync::Mutex<Vec<(u64, usize)>>>,
}

impl Overlay {
    pub fn new(base: std::sync::Arc<Vec<u8>>, logging: bool) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Overlay {
            base,
            pages: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            log: logging.then(|| std::sync::Mutex::new(Vec::new())),
        })
    }

    /// Lay `bytes` over the image at byte `at`; past the end is dropped.
    pub fn patch(&self, at: u64, bytes: &[u8]) {
        let size = self.base.len() as u64;
        let end = at.saturating_add(bytes.len() as u64).min(size);
        if at >= end {
            return;
        }
        let mut pages = self.pages.lock().unwrap_or_else(|p| p.into_inner());
        for p in at / WI_PAGE..=(end - 1) / WI_PAGE {
            let start = p * WI_PAGE;
            let page = pages.entry(p).or_insert_with(|| {
                let to = (start + WI_PAGE).min(size);
                self.base[start as usize..to as usize].to_vec()
            });
            let from = at.max(start);
            let to = end.min(start + page.len() as u64);
            page[(from - start) as usize..(to - start) as usize]
                .copy_from_slice(&bytes[(from - at) as usize..(to - at) as usize]);
        }
    }

    /// The image's bytes `at..at + len` as a read sees them now.
    pub fn bytes(&self, at: u64, len: usize) -> Vec<u8> {
        let mut out = vec![0u8; len];
        let _ = self.read_into(at, &mut out);
        out
    }

    /// The reads logged so far.
    pub fn reads(&self) -> Vec<(u64, usize)> {
        match &self.log {
            Some(l) => l.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            None => Vec::new(),
        }
    }

    fn read_into(&self, offset: u64, buf: &mut [u8]) -> fs_core::Result<()> {
        let size = self.base.len() as u64;
        let end = offset
            .checked_add(buf.len() as u64)
            .filter(|&e| e <= size)
            .ok_or(fs_core::Error::OutOfBounds {
                offset,
                len: buf.len() as u64,
                size,
            })?;
        if buf.is_empty() {
            return Ok(());
        }
        buf.copy_from_slice(&self.base[offset as usize..end as usize]);
        let pages = self.pages.lock().unwrap_or_else(|p| p.into_inner());
        for (&p, page) in pages.range(offset / WI_PAGE..=(end - 1) / WI_PAGE) {
            let start = p * WI_PAGE;
            let from = offset.max(start);
            let to = end.min(start + page.len() as u64);
            if from < to {
                buf[(from - offset) as usize..(to - offset) as usize]
                    .copy_from_slice(&page[(from - start) as usize..(to - start) as usize]);
            }
        }
        Ok(())
    }
}

impl fs_core::BlockRead for Overlay {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> fs_core::Result<()> {
        if let Some(l) = &self.log {
            l.lock()
                .unwrap_or_else(|p| p.into_inner())
                .push((offset, buf.len()));
        }
        self.read_into(offset, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.base.len() as u64
    }
}

impl fs_core::BlockDevice for Overlay {
    fn write_at(&self, offset: u64, buf: &[u8]) -> fs_core::Result<()> {
        let size = self.base.len() as u64;
        if offset
            .checked_add(buf.len() as u64)
            .is_none_or(|e| e > size)
        {
            return Err(fs_core::Error::OutOfBounds {
                offset,
                len: buf.len() as u64,
                size,
            });
        }
        self.patch(offset, buf);
        Ok(())
    }

    fn is_writable(&self) -> bool {
        true
    }
}

/// How far the walk goes: files and directories visited, and entries of
/// each directory followed. A mutated size or count must not make one case
/// take minutes.
const WI_WALK_NODES: usize = 48;
const WI_WALK_ENTRIES: usize = 16;

/// Open, walk, check; on an image the checker passes, and when `write`
/// says so, a few writes, then the walk and the checker again. Every error
/// is an answer; only a panic, a hang or an allocation past the cap is a
/// finding.
pub fn workload(dev: &std::sync::Arc<Overlay>, write: bool) {
    let Ok(fs) = fs_bcachefs::Filesystem::open(std::sync::Arc::clone(dev)) else {
        return;
    };
    wi_walk(&fs);
    drop(fs);
    let clean = matches!(fs_bcachefs::check::check(&**dev), Ok(r) if r.clean());
    if !(write && clean) {
        return;
    }
    wi_writes(dev);
    if let Ok(fs) = fs_bcachefs::Filesystem::open(std::sync::Arc::clone(dev)) {
        wi_walk(&fs);
    }
    let _ = fs_bcachefs::check::check(&**dev);
}

fn wi_walk<D: fs_core::BlockRead>(fs: &fs_bcachefs::Filesystem<D>) {
    let _ = fs.usage();
    let mut queue = vec![String::from("/")];
    let mut seen = 0;
    while let Some(path) = queue.pop() {
        if seen == WI_WALK_NODES {
            break;
        }
        seen += 1;
        let Ok(n) = fs.resolve(&path) else { continue };
        let _ = fs.xattrs_at(n);
        let Ok(i) = fs.inode_at(n) else { continue };
        match i.mode & 0o170000 {
            0o040000 => {
                if let Ok(entries) = fs.readdir_at(n) {
                    for e in entries.iter().take(WI_WALK_ENTRIES) {
                        if let Ok(name) = std::str::from_utf8(&e.name) {
                            let sep = if path.ends_with('/') { "" } else { "/" };
                            queue.push(format!("{path}{sep}{name}"));
                        }
                    }
                }
            }
            0o100000 | 0o120000 => {
                let _ = fs.read_range_at(n, 0, i.size.min(64 << 10) as usize);
                let _ = fs.read_range_at(n, i.size / 2, 4096);
            }
            _ => {}
        }
    }
}

fn wi_writes(dev: &std::sync::Arc<Overlay>) {
    let Ok(mut w) = fs_bcachefs::write::Writer::open(std::sync::Arc::clone(dev)) else {
        return;
    };
    let root = fs_bcachefs::inode::ROOT_INO;
    let data: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
    if let Ok(ino) = w.create_file(root, b"whole-image-fuzz", &data, 0o644) {
        let _ = w.write_at(ino, 1000, b"whole image");
        let _ = w.set_xattr(ino, b"user.fuzz", b"value");
        let _ = w.truncate(ino, 100);
    }
    let _ = w.mkdir(root, b"whole-image-fuzz-dir", 0o755);
    let _ = w.unlink(root, b"whole-image-fuzz");
}

/// What a case may mutate: the btree nodes, journal entries and the
/// superblock a clean pass read, as `(byte offset, length)`.
pub struct Targets {
    pub nodes: Vec<(u64, usize)>,
    pub jsets: Vec<(u64, usize)>,
    pub superblock: Option<(u64, usize)>,
    /// The btrees with a root, for key-level cases.
    pub btrees: Vec<u8>,
    block: usize,
}

/// The primary superblock's byte offset (S1: sector 8).
const WI_SB_AT: u64 = 4096;

/// Run the workload once over the fixture unchanged, logging its reads,
/// and find the structures among them: a btree node or a journal entry is
/// recognised by its magic at byte 16 of any block it read.
pub fn targets(base: &std::sync::Arc<Vec<u8>>) -> Targets {
    let dev = Overlay::new(std::sync::Arc::clone(base), true);
    workload(&dev, false);
    let mut t = Targets {
        nodes: Vec::new(),
        jsets: Vec::new(),
        superblock: None,
        btrees: Vec::new(),
        block: 512,
    };
    let Ok(sb) = Superblock::read(&*dev) else {
        return t;
    };
    t.block = (usize::from(sb.block_size) * 512).max(512);
    t.btrees = sb
        .btree_roots()
        .map(|r| r.iter().map(|r| r.btree_id).collect())
        .unwrap_or_default();
    let node_bytes = sb.btree_node_size() as usize * 512;
    let node_magic = fs_bcachefs::btree::node_magic(&sb.uuid);
    let jset_magic = fs_bcachefs::journal::jset_magic(&sb.uuid);
    let size = base.len() as u64;
    let block = t.block as u64;
    let (mut nodes, mut jsets) = (
        std::collections::BTreeSet::new(),
        std::collections::BTreeSet::new(),
    );
    for (at, len) in dev.reads() {
        let mut p = at.div_ceil(block) * block;
        while p + 64 <= (at + len as u64).min(size) {
            let magic = le64(base, p as usize + 16);
            if magic == node_magic && node_bytes > 0 && p + node_bytes as u64 <= size {
                nodes.insert((p, node_bytes));
            } else if magic == jset_magic {
                let u64s = le32(base, p as usize + 40) as u64;
                let n = (56 + u64s * 8).div_ceil(block) * block;
                if p + n <= size {
                    jsets.insert((p, n as usize));
                }
            }
            p += block;
        }
        if at <= WI_SB_AT && WI_SB_AT < at + len as u64 {
            let n = SB_HEADER_BYTES + le32(base, WI_SB_AT as usize + 0x7c) as usize * 8;
            if WI_SB_AT + n as u64 <= size {
                t.superblock = Some((WI_SB_AT, n));
            }
        }
    }
    t.nodes = nodes.into_iter().collect();
    t.jsets = jsets.into_iter().collect();
    t
}

/// xorshift64*, as the decoder gate's: small, deterministic, no dependency.
pub struct WiRng(u64);

impl WiRng {
    pub fn new(seed: u64) -> Self {
        WiRng(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next_u64() % bound as u64) as usize
        }
    }
}

/// One change to a structure's bytes, keeping its length: flipped bits, a
/// run of zeros or ones, an extreme value in a field, or a field nudged by
/// a little (the decoder gate's operations, for the same reasons).
fn wi_mutate(b: &mut [u8], rng: &mut WiRng) {
    if b.len() < 8 {
        return;
    }
    match rng.below(4) {
        0 => {
            for _ in 0..=rng.below(4) {
                let at = rng.below(b.len());
                b[at] ^= 1u8 << rng.below(8);
            }
        }
        1 => {
            let at = rng.below(b.len());
            let len = 1 + rng.below(16.min(b.len() - at));
            b[at..at + len].fill(if rng.next_u64() & 1 == 0 { 0 } else { 0xff });
        }
        2 => {
            let width = [2usize, 4, 8][rng.below(3)];
            let at = rng.below(b.len() - width + 1) & !(width - 1);
            let value: u64 = match rng.below(4) {
                0 => 0,
                1 => 1,
                2 => u64::MAX,
                _ => rng.next_u64(),
            };
            b[at..at + width].copy_from_slice(&value.to_le_bytes()[..width]);
        }
        _ => {
            let at = rng.below(b.len() / 4) * 4;
            let word = u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"));
            let delta = [1i64, -1, 2, -2, 255, -255][rng.below(6)];
            let changed = (i64::from(word).wrapping_add(delta)) as u32;
            b[at..at + 4].copy_from_slice(&changed.to_le_bytes());
        }
    }
}

/// Lay one case over `dev`, chosen by `seed`, and say what it was: a
/// structure's bytes changed (and re-stamped, three times in four), or one
/// field of one real key changed and committed through the writer.
pub fn apply_case(dev: &std::sync::Arc<Overlay>, t: &Targets, seed: u64) -> String {
    let mut rng = WiRng::new(seed);
    if rng.below(3) == 0 && !t.btrees.is_empty() {
        return wi_key_case(dev, t, &mut rng);
    }
    let pick = rng.below(20);
    let (kind, (at, len)) = if let (true, Some(sb)) = (pick < 3, t.superblock) {
        ("superblock", sb)
    } else if pick < 8 && !t.jsets.is_empty() {
        ("jset", t.jsets[rng.below(t.jsets.len())])
    } else if !t.nodes.is_empty() {
        ("node", t.nodes[rng.below(t.nodes.len())])
    } else {
        return String::from("nothing to mutate");
    };
    let mut b = dev.bytes(at, len);
    wi_mutate(&mut b, &mut rng);
    let stamped = rng.below(4) != 0;
    if stamped {
        match kind {
            "superblock" => restamp_superblock(&mut b),
            "jset" => restamp_jset(&mut b),
            _ => restamp_node(&mut b, t.block),
        }
    }
    dev.patch(at, &b);
    format!(
        "{kind} at byte {at} ({len} bytes){}",
        if stamped { ", re-stamped" } else { ", torn" }
    )
}

/// One field of one real key changed, and the key committed with
/// `Writer::insert` into the btree it came from.
fn wi_key_case(dev: &std::sync::Arc<Overlay>, t: &Targets, rng: &mut WiRng) -> String {
    let id = t.btrees[rng.below(t.btrees.len())];
    let Ok(sb) = Superblock::read(&**dev) else {
        return String::from("no superblock");
    };
    let Ok(keys) = fs_bcachefs::btree::walk(&**dev, &sb, id) else {
        return format!("btree {id} unreadable");
    };
    if keys.is_empty() {
        return format!("btree {id} empty");
    }
    let mut k = keys[rng.below(keys.len())].clone();
    let what = match rng.below(6) {
        0 => {
            k.size = [0u32, 1, u32::MAX, k.size.wrapping_add(1)][rng.below(4)];
            "size"
        }
        1 => {
            k.pos.offset = [0u64, u64::MAX, k.pos.offset.wrapping_add(1), k.pos.offset.wrapping_sub(1)]
                [rng.below(4)];
            "offset"
        }
        2 => {
            k.pos.inode = [0u64, u64::MAX, k.pos.inode.wrapping_add(1)][rng.below(3)];
            "inode"
        }
        3 => {
            k.pos.snapshot = [0u32, 1, u32::MAX - 1, k.pos.snapshot.wrapping_add(1)][rng.below(4)];
            "snapshot"
        }
        4 => {
            k.key_type = rng.below(40) as u8;
            "type"
        }
        _ => {
            if k.value.len() >= 8 {
                let at = rng.below(k.value.len() / 8) * 8;
                let v: u64 = [0u64, 1, u64::MAX, rng.next_u64()][rng.below(4)];
                k.value[at..at + 8].copy_from_slice(&v.to_le_bytes());
            } else {
                k.value.truncate(rng.below(k.value.len() + 1));
            }
            "value"
        }
    };
    let pos = k.pos;
    let Ok(mut w) = fs_bcachefs::write::Writer::open(std::sync::Arc::clone(dev)) else {
        return format!("btree {id}: the writer refused to open");
    };
    let r = w.insert(id, vec![k]);
    format!(
        "btree {id} key at {pos}: {what} changed, insert {}",
        if r.is_ok() { "committed" } else { "refused" }
    )
}

/// The fixtures cases are laid over: the reference formatter's default and
/// 4 KiB-block images, an lz4 one, the aged image (two-level btrees, reused
/// buckets, pieces of checksummed regions), and the kernel module's (journal
/// entries, snapshots, subvolumes, reflinks).
pub const WI_FIXTURES: [&str; 7] = [
    "default.img",
    "block4k.img",
    "lz4.img",
    "aged.img",
    "kernel.img",
    "kernel-rootsnap.img",
    "kernel-written.img",
];

/// A case as the explorer is given it, and as a reproducer is committed: a
/// byte choosing the fixture (modulo the list), then the seed, little-endian.
pub fn wi_decode(data: &[u8]) -> Option<(usize, u64)> {
    let (&i, rest) = data.split_first()?;
    let seed = u64::from_le_bytes(rest.get(..8)?.try_into().ok()?);
    Some((usize::from(i), seed))
}
