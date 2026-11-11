//! Measures the whole-drive index: `ixbench <root>... ` prints scan, save, load and query times.
use rx_core::index::Index;
use std::time::Instant;

fn main() {
    let roots: Vec<std::path::PathBuf> = std::env::args().skip(1).map(Into::into).collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let t = Instant::now();
    let ix = Index::scan(&roots, threads, || ());
    println!("skan: {} pozycji w {:.1} s ({threads} wątków)", ix.len(), t.elapsed().as_secs_f64());
    let file = std::env::temp_dir().join("rx-ixbench.bin");
    let t = Instant::now();
    ix.save(&file).unwrap();
    let size = std::fs::metadata(&file).unwrap().len();
    println!("zapis: {:.2} s, plik {} MB", t.elapsed().as_secs_f64(), size >> 20);
    drop(ix);
    let t = Instant::now();
    let ix = Index::load(&file).unwrap();
    println!("odczyt: {:.2} s", t.elapsed().as_secs_f64());
    for q in ["e", "faktura", "*.pdf", "zolw", "raport 2025", "xqzv"] {
        let t = Instant::now();
        let (hits, total) = ix.search(q, 5000);
        println!("szukaj {q:>12}: {total:>8} trafień, {:>6.1} ms", t.elapsed().as_secs_f64() * 1000.0);
        let _ = hits;
    }
    let _ = std::fs::remove_file(file);
}
