//! `cargo run --release -p lightcraft-raw --example crxdump -- FILE.CR3 [--hex N]`
//!
//! Analysis helper for the CR3 raw codec: lists the tracks, prints the full-size raw's `CMP1` coding header and
//! `IAD1` areas, and hex-dumps the first `N` bytes (default: the header size from `CMP1`, at most 2048) of its
//! sample with the `ffXX` marker candidates of the tile / plane / subband headers flagged.
use lightcraft_meta::cr3::{Cr3TrackKind, parse_cr3};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let hex_len = args.iter().position(|a| a == "--hex").and_then(|i| args.get(i + 1)).and_then(|n| n.parse::<usize>().ok());
    for path in args.iter().filter(|a| a.ends_with(".CR3") || a.ends_with(".cr3")) {
        let bytes = std::fs::read(path).expect("read");
        let Some(cr3) = parse_cr3(&bytes) else {
            println!("{path}: not a CR3 file");
            continue;
        };
        println!("{path}: {} bytes, {} tracks", bytes.len(), cr3.tracks.len());
        for (i, t) in cr3.tracks.iter().enumerate() {
            match &t.kind {
                Cr3TrackKind::Raw { width, height, .. } => {
                    println!(
                        "  track {i}: raw {width}x{height} sample {:?}\n    {:?}\n    {:?}",
                        t.data,
                        t.compression(&bytes),
                        t.image_area(&bytes)
                    );
                }
                other => println!("  track {i}: {other:?} sample {:?}", t.data),
            }
        }
        // the full-size raw: the largest raw track with a coding header (the reduced raw is one too)
        let full = cr3.tracks.iter().filter_map(|t| Some((t.data?, t.compression(&bytes)?))).max_by_key(|(_, c)| c.width as u64 * c.height as u64);
        let Some(((at, len), c)) = full else { continue };
        let sample = &bytes[at..at + len];
        let n = hex_len.unwrap_or((c.header_size as usize).min(2048)).min(sample.len());
        println!("  full-size raw sample: {len} bytes at {at}; first {n}:");
        for (row, chunk) in sample[..n].chunks(32).enumerate() {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
            println!("    {:5}  {}", row * 32, hex.join(" "));
        }
        let markers: Vec<String> = sample[..n]
            .windows(2)
            .enumerate()
            .filter(|(_, w)| w[0] == 0xff && matches!(w[1], 0x01..=0x03 | 0x11..=0x13))
            .map(|(i, w)| format!("ff{:02x}@{i}", w[1]))
            .collect();
        println!("  marker candidates: {}", markers.join(" "));
    }
}
