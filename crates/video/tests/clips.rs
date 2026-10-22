//! Real clips from a folder (`LIGHTCRAFT_VIDEO_CLIPS=<folder of .mp4 / .mov / .m4v>`; nothing is committed): each is probed,
//! given a poster, read at its middle and read for a second in order. Skipped without the variable.

use std::path::PathBuf;

use lightcraft_video::{Clip, probe_path};

fn mean(img: &lightcraft_raster::Rgb32f) -> f32 {
    img.data.iter().map(|p| (p[0] + p[1] + p[2]) / 3.0).sum::<f32>() / img.data.len().max(1) as f32
}

#[test]
fn real_clips_decode() {
    let Some(dir) = std::env::var_os("LIGHTCRAFT_VIDEO_CLIPS").map(PathBuf::from) else {
        lightcraft_testkit::skip!(Input, "set LIGHTCRAFT_VIDEO_CLIPS to a folder of .mp4 / .mov / .m4v clips");
        return;
    };
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let path = entry.path();
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !["mp4", "mov", "m4v"].contains(&ext.as_str()) {
            continue;
        }
        seen += 1;
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let info = probe_path(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        eprintln!("{name}: {}x{} {} {:.1} s {:.1} fps, sound {}", info.width, info.height, info.codec, info.duration, info.fps, info.has_audio);
        if info.unsupported.is_some() {
            lightcraft_testkit::skip!(Feature, "{name}: {}", info.unsupported.clone().unwrap_or_default());
            continue;
        }
        let clip = Clip::open_path(&path).unwrap();
        let poster = clip.poster(512).unwrap_or_else(|e| panic!("{name}: poster: {e}"));
        assert!(poster.width.max(poster.height) >= 256 && mean(&poster) > 0.01, "{name}: poster {}x{} mean {}", poster.width, poster.height, mean(&poster));
        let middle = clip.frame_at(info.duration / 2.0, 512).unwrap_or_else(|e| panic!("{name}: middle: {e}"));
        assert!(mean(&middle) > 0.0, "{name}");
        let (mut times, mut got) = (Vec::new(), 0);
        clip.for_each_frame(0.0, 1.0, 256, |t, _| {
            times.push(t);
            got += 1;
            Ok(true)
        })
        .unwrap_or_else(|e| panic!("{name}: frames: {e}"));
        assert!(got as f64 >= info.fps * 0.9 && (got as f64) <= info.fps * 1.1 + 1.0, "{name}: {got} pictures in a second at {} fps", info.fps);
        assert!(times.windows(2).all(|w| w[0] < w[1]), "{name}: {times:?}");
    }
    assert!(seen > 0, "no clips in {}", dir.display());
}
