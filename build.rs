//! Puts assets/traymouse.ico into the exe as its icon.
//! Writes the COFF object a resource compiler (windres / rc + cvtres) would make, so none has to be installed;
//! both the GNU and MSVC linkers merge its .rsrc section into the exe.

use std::path::PathBuf;

const RT_ICON: u32 = 3;
const RT_GROUP_ICON: u32 = 14;
const LANG_EN_US: u32 = 0x0409;

fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// .ico file -> (type, id, data) resources: one RT_ICON per image plus the RT_GROUP_ICON that lists them.
fn ico_resources(ico: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    assert!(ico.len() >= 6 && u16le(ico, 2) == 1, "not an .ico file");
    let count = u16le(ico, 4) as usize;
    let mut out = Vec::new();
    let mut group = vec![0, 0, 1, 0];
    group.extend((count as u16).to_le_bytes());
    for i in 0..count {
        let e = &ico[6 + 16 * i..6 + 16 * (i + 1)];
        let (size, offset) = (u32le(e, 8) as usize, u32le(e, 12) as usize);
        let id = i as u16 + 1;
        out.push((RT_ICON, id as u32, ico[offset..offset + size].to_vec()));
        group.extend(&e[..12]); // width, height, colors, reserved, planes, bit count, bytes in image
        group.extend(id.to_le_bytes());
    }
    out.push((RT_GROUP_ICON, 1, group));
    out
}

/// A COFF object whose .rsrc section holds the resource tree (type -> id -> language -> data).
/// `res` must be sorted by (type, id); data addresses are filled in by the linker (ADDR32NB relocations).
fn coff(res: &[(u32, u32, Vec<u8>)]) -> Vec<u8> {
    let types: Vec<u32> = res.iter().map(|r| r.0).fold(Vec::new(), |mut v, t| {
        if v.last() != Some(&t) {
            v.push(t)
        }
        v
    });
    let dir = |n: usize| 16 + 8 * n;
    let header = |n: u16| {
        let mut h = vec![0u8; 12];
        h.extend(0u16.to_le_bytes()); // named entries
        h.extend(n.to_le_bytes()); // id entries
        h
    };
    // layout: root dir, one dir per type, one language dir per resource, data entries, then the data
    let type_dirs = dir(types.len());
    let mut id_dir_at = Vec::new();
    let mut at = type_dirs;
    for t in &types {
        id_dir_at.push(at);
        at += dir(res.iter().filter(|r| r.0 == *t).count());
    }
    let lang_dir_at: Vec<usize> = (0..res.len()).map(|i| at + i * dir(1)).collect();
    at += res.len() * dir(1);
    let data_entry_at: Vec<usize> = (0..res.len()).map(|i| at + i * 16).collect();
    at += res.len() * 16;
    let mut blob_at = Vec::new();
    for r in res {
        at = at.next_multiple_of(8);
        blob_at.push(at);
        at += r.2.len();
    }

    let mut sec = header(types.len() as u16);
    for (t, d) in types.iter().zip(&id_dir_at) {
        sec.extend(t.to_le_bytes());
        sec.extend((*d as u32 | 0x8000_0000).to_le_bytes());
    }
    for t in &types {
        let ids: Vec<usize> = (0..res.len()).filter(|&i| res[i].0 == *t).collect();
        sec.extend(header(ids.len() as u16));
        for i in ids {
            sec.extend(res[i].1.to_le_bytes());
            sec.extend((lang_dir_at[i] as u32 | 0x8000_0000).to_le_bytes());
        }
    }
    for d in &data_entry_at {
        sec.extend(header(1));
        sec.extend(LANG_EN_US.to_le_bytes());
        sec.extend((*d as u32).to_le_bytes());
    }
    let mut relocs = Vec::new();
    for (r, b) in res.iter().zip(&blob_at) {
        relocs.push(sec.len() as u32);
        sec.extend((*b as u32).to_le_bytes()); // offset in .rsrc; linker adds the section's address
        sec.extend((r.2.len() as u32).to_le_bytes());
        sec.extend([0u8; 8]); // code page, reserved
    }
    for (r, b) in res.iter().zip(&blob_at) {
        sec.resize(*b, 0);
        sec.extend(&r.2);
    }
    sec.resize(sec.len().next_multiple_of(4), 0);

    let raw_at = 20 + 40;
    let reloc_at = raw_at + sec.len();
    let sym_at = reloc_at + relocs.len() * 10;
    let mut o = Vec::new();
    o.extend(0x8664u16.to_le_bytes()); // x86-64
    o.extend(1u16.to_le_bytes()); // sections
    o.extend(0u32.to_le_bytes()); // timestamp
    o.extend((sym_at as u32).to_le_bytes());
    o.extend(1u32.to_le_bytes()); // symbols
    o.extend(0u16.to_le_bytes()); // optional header size
    o.extend(0u16.to_le_bytes()); // characteristics
    o.extend(*b".rsrc\0\0\0");
    o.extend(0u32.to_le_bytes()); // virtual size
    o.extend(0u32.to_le_bytes()); // virtual address
    o.extend((sec.len() as u32).to_le_bytes());
    o.extend((raw_at as u32).to_le_bytes());
    o.extend((reloc_at as u32).to_le_bytes());
    o.extend(0u32.to_le_bytes()); // line numbers
    o.extend((relocs.len() as u16).to_le_bytes());
    o.extend(0u16.to_le_bytes());
    o.extend(0x4000_0040u32.to_le_bytes()); // initialized data, readable
    o.extend(&sec);
    for r in relocs {
        o.extend(r.to_le_bytes());
        o.extend(0u32.to_le_bytes()); // symbol 0: the .rsrc section
        o.extend(3u16.to_le_bytes()); // IMAGE_REL_AMD64_ADDR32NB
    }
    o.extend(*b".rsrc\0\0\0");
    o.extend(0u32.to_le_bytes()); // value
    o.extend(1i16.to_le_bytes()); // section 1
    o.extend(0u16.to_le_bytes()); // type
    o.push(3); // static
    o.push(0); // aux symbols
    o.extend(4u32.to_le_bytes()); // empty string table
    o
}

fn main() {
    println!("cargo:rerun-if-changed=assets/traymouse.ico");
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.starts_with("x86_64-pc-windows") {
        return; // shortcut: x86-64 Windows only (the only build we ship), other targets just get no icon
    }
    let ico = std::fs::read("assets/traymouse.ico").expect("assets/traymouse.ico");
    let obj = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("traymouse_icon.o");
    std::fs::write(&obj, coff(&ico_resources(&ico))).unwrap();
    println!("cargo:rustc-link-arg-bins={}", obj.display());
}
