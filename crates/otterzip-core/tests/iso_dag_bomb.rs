//! C4 regression: ISO directory walk must be bounded by node count, not just
//! depth. A crafted DAG — N chained extents, each with two records pointing at
//! the SAME next extent — has depth N but 2^N reachable paths. The depth cap
//! (64) never fires; before the fix a ~120 KB image walked ~2^40 nodes and
//! never returned. Detection is extension-only, so merely opening the file
//! reaches this. `visited: HashSet<extent_lba>` collapses the DAG to linear.

use std::io::Write;
use std::time::{Duration, Instant};

use otterzip_core::{Archive, OpenMode};

const SECTOR: usize = 2048;

fn w_both_u32(dst: &mut [u8], v: u32) {
    dst[0..4].copy_from_slice(&v.to_le_bytes());
    dst[4..8].copy_from_slice(&v.to_be_bytes());
}
fn w_both_u16(dst: &mut [u8], v: u16) {
    dst[0..2].copy_from_slice(&v.to_le_bytes());
    dst[2..4].copy_from_slice(&v.to_be_bytes());
}
fn w_dir(data: &mut [u8], off: usize, lba: u32, size: u32, flags: u8, name: &[u8]) -> usize {
    let mut len = 33 + name.len();
    if len % 2 != 0 {
        len += 1;
    }
    data[off] = len as u8;
    w_both_u32(&mut data[off + 2..], lba);
    w_both_u32(&mut data[off + 10..], size);
    data[off + 25] = flags;
    data[off + 32] = name.len() as u8;
    data[off + 33..off + 33 + name.len()].copy_from_slice(name);
    len
}

fn build_dag_bomb(depth: u32) -> Vec<u8> {
    let root_lba = 18u32;
    let first_level_lba = 19u32;
    let total = first_level_lba + depth + 1;
    let mut data = vec![0u8; total as usize * SECTOR];

    let pvd = 16 * SECTOR;
    data[pvd] = 1;
    data[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
    data[pvd + 6] = 1;
    data[pvd + 156] = 34;
    w_both_u32(&mut data[pvd + 158..], root_lba);
    w_both_u32(&mut data[pvd + 166..], SECTOR as u32);
    data[pvd + 156 + 25] = 0x02;
    data[pvd + 156 + 32] = 1;
    data[pvd + 156 + 33] = 0;
    w_both_u32(&mut data[pvd + 80..], total);
    w_both_u16(&mut data[pvd + 128..], SECTOR as u16);

    let term = 17 * SECTOR;
    data[term] = 255;
    data[term + 1..term + 6].copy_from_slice(b"CD001");
    data[term + 6] = 1;

    let root = root_lba as usize * SECTOR;
    let mut o = root;
    o += w_dir(&mut data, o, root_lba, SECTOR as u32, 0x02, b"\0");
    o += w_dir(&mut data, o, root_lba, SECTOR as u32, 0x02, b"\x01");
    o += w_dir(&mut data, o, first_level_lba, SECTOR as u32, 0x02, b"A");
    let _ = w_dir(&mut data, o, first_level_lba, SECTOR as u32, 0x02, b"B");

    for lvl in 1..=depth {
        let lba = first_level_lba + (lvl - 1);
        let base = lba as usize * SECTOR;
        let mut oo = base;
        oo += w_dir(&mut data, oo, lba, SECTOR as u32, 0x02, b"\0");
        oo += w_dir(&mut data, oo, lba, SECTOR as u32, 0x02, b"\x01");
        if lvl == depth {
            let _ = w_dir(&mut data, oo, lba, 4, 0x00, b"LEAF");
        } else {
            let next = first_level_lba + lvl;
            oo += w_dir(&mut data, oo, next, SECTOR as u32, 0x02, b"A");
            let _ = w_dir(&mut data, oo, next, SECTOR as u32, 0x02, b"B");
        }
    }
    data
}

#[test]
fn c4_iso_dag_bomb_terminates_quickly() {
    // depth 40 = ~2^40 paths if un-deduped (the old behaviour never returned on
    // a ~120 KB file). With extent-dedup it must finish near-instantly on a
    // background thread; 5 s is a very generous ceiling.
    let depth = 40u32;
    let bytes = build_dag_bomb(depth);
    let td = tempfile::tempdir().unwrap();
    let p = td.path().join("bomb.iso");
    std::fs::File::create(&p).unwrap().write_all(&bytes).unwrap();

    let p2 = p.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let t = Instant::now();
        let r = Archive::open(&p2, OpenMode::Read).and_then(|a| a.entries().map(|it| it.count()));
        let _ = tx.send((t.elapsed(), r));
    });

    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok((dur, res)) => {
            println!("depth {depth}, {} B, returned in {dur:?}: {res:?}", bytes.len());
            // A deduped walk visits each of the ~41 extents once — a few dozen
            // entries, not billions. Assert it is small, proving no blow-up.
            let n = res.unwrap();
            assert!(n < 10_000, "entry count {n} suggests the DAG was not deduped");
        }
        Err(_) => panic!("Archive::open on a {}-byte DAG-bomb ISO did not return in 5 s", bytes.len()),
    }
}

#[test]
fn c4_iso_dag_scaling_is_linear_after_fix() {
    // Before the fix these grew 2^depth. After, each is a handful of extents.
    for depth in [8u32, 16, 24] {
        let bytes = build_dag_bomb(depth);
        let td = tempfile::tempdir().unwrap();
        let p = td.path().join("b.iso");
        std::fs::File::create(&p).unwrap().write_all(&bytes).unwrap();
        let t = Instant::now();
        let n = Archive::open(&p, OpenMode::Read)
            .and_then(|a| a.entries().map(|it| it.count()))
            .unwrap_or(0);
        println!("  depth {depth:2}  entries {n:>4}  {:?}", t.elapsed());
        assert!(n < 10_000, "depth {depth}: {n} entries — not deduped");
    }
}
