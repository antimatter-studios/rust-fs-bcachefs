fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dev = fs_core::FileDevice::open(&a[1]).unwrap();
    let sb = fs_bcachefs::superblock::Superblock::read(&dev).unwrap();
    let want: u64 = a[2].parse().unwrap();
    for k in fs_bcachefs::btree::walk(&dev, &sb, a[3].parse().unwrap()).unwrap() {
        if k.pos.offset == want || k.pos.inode == want {
            let hex: Vec<String> = k.value.iter().map(|b| format!("{b:02x}")).collect();
            println!(
                "{} type {} size {}: {}",
                k.pos,
                k.key_type,
                k.size,
                hex.join(" ")
            );
        }
    }
}
