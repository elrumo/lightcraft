//! What does `lightcraft_video::Clip::poster(512)` cost, split into opening the file and making the poster?
//! (docs/video.md, "Hardware video", follow-ups.)
//!
//! `cargo run --release --example poster_profile -- CLIP.mp4...` (the clips `hw_agent_bench` writes to the system temp
//! folder, `lightcraft-hw-bench/`). Median of 10 of wall-clock and process CPU time (all threads). Set `LIGHTCRAFT_PROFILE=1`
//! for the stages inside a poster on stderr, and `RAYON_NUM_THREADS=1` to make their wall time equal their CPU time.

use std::time::Instant;

use cpu_time::ProcessTime;
use lightcraft_video::Clip;

const REPS: usize = 10;

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(0.0)
}

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("poster_profile: give it clips: poster_profile CLIP.mp4...");
        std::process::exit(2);
    }
    println!("| clip | open wall · CPU ms | poster wall · CPU ms | picture |\n|---|---|---|---|");
    for path in &paths {
        let (mut open_w, mut open_c, mut post_w, mut post_c) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut size = (0, 0);
        for _ in 0..REPS {
            let (w, c) = (Instant::now(), ProcessTime::now());
            let clip = match Clip::open_path(std::path::Path::new(path)) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("{path}: {e}");
                    std::process::exit(1);
                }
            };
            open_w.push(w.elapsed().as_secs_f64() * 1e3);
            open_c.push(c.elapsed().as_secs_f64() * 1e3);
            let (w, c) = (Instant::now(), ProcessTime::now());
            match clip.poster(512) {
                Ok(img) => size = (img.width, img.height),
                Err(e) => {
                    eprintln!("{path}: {e}");
                    std::process::exit(1);
                }
            }
            post_w.push(w.elapsed().as_secs_f64() * 1e3);
            post_c.push(c.elapsed().as_secs_f64() * 1e3);
        }
        let name = std::path::Path::new(path).file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        println!(
            "| {name} | {:.1} · {:.1} | {:.1} · {:.1} | {}x{} |",
            median(open_w),
            median(open_c),
            median(post_w),
            median(post_c),
            size.0,
            size.1
        );
    }
}
