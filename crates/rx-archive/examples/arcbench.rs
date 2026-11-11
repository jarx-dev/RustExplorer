//! `arcbench <archive>`: unpacks into a temp folder, packs that back to .zip and .7z, unpacks both
//! again and prints times and counts; everything it made is removed at the end.
use std::time::Instant;

fn main() {
    let src = std::path::PathBuf::from(std::env::args().nth(1).expect("ścieżka do archiwum"));
    let tmp = std::env::temp_dir().join(format!("rx-arcbench-{}", std::process::id()));
    // extract makes only its own folder, so the one around it must be there
    std::fs::create_dir_all(&tmp).unwrap();
    let t = Instant::now();
    // an encrypted archive's password comes from RX_PW, never from the command line
    let pw = std::env::var("RX_PW").ok();
    let r = rx_archive::extract(&src, &tmp.join("out"), pw.as_deref(), &mut |_, _| {});
    println!("wypakowanie {}: {r:?} w {:.2} s", src.display(), t.elapsed().as_secs_f64());
    if r.is_err() {
        return;
    }
    let items: Vec<_> = std::fs::read_dir(tmp.join("out")).unwrap().flatten().map(|e| e.path()).collect();
    for ext in ["zip", "7z"] {
        let arc = tmp.join(format!("paczka.{ext}"));
        let t = Instant::now();
        let n = rx_archive::pack(&items, &tmp.join("out"), &arc,&mut |_, _| {});
        let size = std::fs::metadata(&arc).map(|m| m.len()).unwrap_or(0);
        println!("pakowanie {ext}: {n:?}, {size} B w {:.2} s", t.elapsed().as_secs_f64());
        let t = Instant::now();
        let back = rx_archive::extract(&arc, &tmp.join(format!("back-{ext}")), None, &mut |_, _| {});
        println!("rozpakowanie {ext}: {back:?} w {:.2} s", t.elapsed().as_secs_f64());
    }
    for e in std::fs::read_dir(tmp.join("out")).unwrap().flatten() {
        let m = e.metadata().unwrap();
        println!("  {} {} B, zmieniony {:?}", e.file_name().to_string_lossy(), m.len(), m.modified().ok().map(chrono::DateTime::<chrono::Local>::from).map(|d| d.format("%d.%m.%Y %H:%M").to_string()));
    }
    std::fs::remove_dir_all(&tmp).unwrap();
}
