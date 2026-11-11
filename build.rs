//! The program icon and the application manifest in the EXE; the update identity is built in
//! crates/rx-update.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=ikona.ico");
    println!("cargo:rerun-if-changed=rustexplorer.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let out = std::env::var("OUT_DIR").unwrap();
        if let Err(e) = embed_resources(Path::new(&manifest), Path::new(&out)) {
            println!("cargo:warning=program icon and manifest not embedded: {e}");
        }
    }
}

/// Turns ikona.ico and rustexplorer.manifest into a compiled Windows resource and links it into
/// the program (the Microsoft linker, also lld-link from cargo-xwin, takes a .res file as it is;
/// neither adds a manifest of its own, so this is the only one).
fn embed_resources(manifest: &Path, out: &Path) -> Result<(), String> {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        // shortcut: GNU (mingw) builds get no EXE icon or manifest; add windres here if that matters
        return Err("only the msvc target embeds them".into());
    }
    let xml = std::fs::read(manifest.join("rustexplorer.manifest")).map_err(|e| e.to_string())?;
    let ico = std::fs::read(manifest.join("ikona.ico")).map_err(|e| e.to_string())?;
    if ico.len() < 6 || ico[2] != 1 {
        return Err("not an icon file".into());
    }
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([ico[i], ico[i + 1], ico[i + 2], ico[i + 3]]);

    // The compiled resource format: an empty first entry, one entry per picture, then the
    // group that lists them.
    let mut res = Vec::new();
    let entry = |res: &mut Vec<u8>, kind: u16, id: u16, flags: u16, data: &[u8]| {
        res.extend_from_slice(&(data.len() as u32).to_le_bytes());
        res.extend_from_slice(&32u32.to_le_bytes());
        res.extend_from_slice(&[0xFF, 0xFF]);
        res.extend_from_slice(&kind.to_le_bytes());
        res.extend_from_slice(&[0xFF, 0xFF]);
        res.extend_from_slice(&id.to_le_bytes());
        res.extend_from_slice(&0u32.to_le_bytes());
        res.extend_from_slice(&flags.to_le_bytes());
        res.extend_from_slice(&0x0409u16.to_le_bytes());
        res.extend_from_slice(&0u32.to_le_bytes());
        res.extend_from_slice(&0u32.to_le_bytes());
        res.extend_from_slice(data);
        while res.len() % 4 != 0 {
            res.push(0);
        }
    };
    entry(&mut res, 0, 0, 0, &[]);
    let mut group = vec![0u8, 0, 1, 0];
    group.extend_from_slice(&(count as u16).to_le_bytes());
    for i in 0..count {
        let e = 6 + 16 * i;
        let size = u32_at(e + 8) as usize;
        let at = u32_at(e + 12) as usize;
        let data = ico.get(at..at + size).ok_or("damaged icon file")?;
        entry(&mut res, 3, (i + 1) as u16, 0x1010, data);
        group.extend_from_slice(&ico[e..e + 4]);
        group.extend_from_slice(&u16_at(e + 4).to_le_bytes());
        group.extend_from_slice(&u16_at(e + 6).to_le_bytes());
        group.extend_from_slice(&(size as u32).to_le_bytes());
        group.extend_from_slice(&((i + 1) as u16).to_le_bytes());
    }
    entry(&mut res, 14, 1, 0x1030, &group);
    // RT_MANIFEST 1: the one Windows reads for a program
    entry(&mut res, 24, 1, 0x0030, &xml);
    let res_path = out.join("rustexplorer.res");
    std::fs::write(&res_path, &res).map_err(|e| e.to_string())?;
    println!("cargo:rustc-link-arg-bins={}", res_path.display());
    Ok(())
}
