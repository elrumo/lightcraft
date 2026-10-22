//! Does handing video work to the Mac's media engine pay? (docs/video.md, "Hardware video")
//!
//! Procedural H.264 clips (1080p and 2160p, made with FilmCraft's pure-Rust encoder) go through:
//!   - decode only: software decoder, VideoToolbox decoder;
//!   - encode only: the pure-Rust H.264 encoder, VideoToolbox H.264, VideoToolbox HEVC;
//!   - the whole lighter-copy pipeline (decode, scale on the CPU, encode): A software + software, B VideoToolbox decode +
//!     software encode, C VideoToolbox decode + VideoToolbox encode;
//!   - a poster frame (one key frame): software and VideoToolbox decode, and `lightcraft_video::Clip::poster` as the
//!     product does it today.
//!
//! Every row has the wall clock and the process CPU time (all threads). The media engine's work does not show up as process
//! CPU time, so a hardware path that really uses it shows CPU time far below wall time and far below the software path.
//! The hardware flags are read back, not assumed: `register()` names the backend, `hardware_decoder_for` says whether a
//! session opens for the clip's stream, `hw_stats()` counts the pictures hardware decoders output (and declines and
//! fallbacks), and `VtEncoder::uses_hardware()` reads `UsingHardwareAcceleratedVideoEncoder` from the live session
//! (sessions are created with hardware *required*, so a Mac without an encoder fails instead of falling back silently).
//!
//! Run (macOS): `cargo run --release --example hw_agent_bench -- [--frames 1080=120,2160=60] [--reps 3] [--dir DIR]`.
//! Prints Markdown. Run it on a quiet machine: the wall clock of the software legs depends on free cores (the load average
//! is printed first). For the GPU / media-engine view run `sudo powermetrics --samplers gpu_power,cpu_power -i 1000` in
//! another terminal meanwhile.

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("hw_agent_bench: skipped, it measures VideoToolbox and needs macOS");
}

#[cfg(target_os = "macos")]
fn main() {
    if let Err(e) = bench::run() {
        eprintln!("hw_agent_bench: {e}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "macos")]
mod bench {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};

    use cpu_time::ProcessTime;
    use filmcraft_codecs::{DecodedFrame, hw, make_video_decoder, software_video_decoder};
    use filmcraft_frame::{Chroma, PixelData};
    use filmcraft_h264enc as sw;
    use filmcraft_isobmff::{AvcConfig, Brand, HevcConfig, Mp4File, Mp4Writer, SampleEntry, TrackConfig, WriteSample, WriterOptions};
    use filmcraft_platform::Availability;
    use filmcraft_platform::videotoolbox_encode::{VtConfig, VtEncoder, VtProfile, VtRate};

    type R<T> = Result<T, String>;

    fn s<E: std::fmt::Display>(e: E) -> String {
        e.to_string()
    }

    const FPS: u32 = 30;
    /// Media ticks per picture in the clips we write (timescale = FPS * STEP).
    const STEP: i64 = 1000;

    // ---- measuring -----------------------------------------------------------------------------------------------------

    #[derive(Clone, Copy, Default, Debug)]
    struct Stage {
        wall: Duration,
        cpu: Duration,
    }

    impl Stage {
        fn add(&mut self, o: Stage) {
            self.wall += o.wall;
            self.cpu += o.cpu;
        }
    }

    struct Tick(Instant, ProcessTime);

    fn tick() -> Tick {
        Tick(Instant::now(), ProcessTime::now())
    }

    fn tock(t: Tick) -> Stage {
        Stage { wall: t.0.elapsed(), cpu: t.1.elapsed() }
    }

    fn ms(d: Duration) -> f64 {
        d.as_secs_f64() * 1000.0
    }

    /// The median of `v` (sorted by `key`).
    fn median<T: Copy>(v: &[T], key: impl Fn(&T) -> f64) -> Option<T> {
        let mut v = v.to_vec();
        v.sort_by(|a, b| key(a).total_cmp(&key(b)));
        v.get(v.len() / 2).copied()
    }

    // ---- pictures ------------------------------------------------------------------------------------------------------

    struct Yuv {
        w: usize,
        h: usize,
        y: Vec<u8>,
        u: Vec<u8>,
        v: Vec<u8>,
    }

    /// Tight planar 4:2:0 planes, borrowed.
    struct View<'a> {
        w: usize,
        h: usize,
        y: &'a [u8],
        u: &'a [u8],
        v: &'a [u8],
    }

    impl Yuv {
        fn view(&self) -> View<'_> {
            View { w: self.w, h: self.h, y: &self.y, u: &self.u, v: &self.v }
        }
    }

    fn view_of(f: &DecodedFrame) -> R<View<'_>> {
        let PixelData::Yuv8 { planes, chroma: Chroma::C420, .. } = &f.frame.data else {
            return Err("the bench handles 8-bit 4:2:0 pictures only".into());
        };
        let (w, h) = (f.frame.width as usize, f.frame.height as usize);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        if planes[0].len() < w * h || planes[1].len() < cw * ch || planes[2].len() < cw * ch {
            return Err("the decoder returned padded planes".into());
        }
        Ok(View { w, h, y: &planes[0], u: &planes[1], v: &planes[2] })
    }

    /// A moving, grainy test picture: diagonal bands drift, a box travels across, per-pixel grain keeps the encoders honest.
    fn gen_frame(w: usize, h: usize, t: usize) -> Yuv {
        let (cw, ch) = (w / 2, h / 2);
        let mut y = vec![0u8; w * h];
        let mut u = vec![128u8; cw * ch];
        let mut v = vec![128u8; cw * ch];
        let mut seed = 0x9E37_79B9_7F4A_7C15u64 ^ (t as u64).wrapping_mul(0xD1B5_4A32_D192_ED03);
        let (bw, bh) = (w / 6, h / 6);
        let (bx, by) = ((t * 23) % (w - bw), (t * 11) % (h - bh));
        for row in 0..h {
            let line = &mut y[row * w..row * w + w];
            for (col, px) in line.iter_mut().enumerate() {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let grain = (seed >> 61) as i32 - 4;
                let bands = (((col + row + t * 6) / 24) % 2) as i32 * 28;
                let base = 40 + (col * 150 / w) as i32 + (row * 50 / h) as i32 + bands;
                let inside = col >= bx && col < bx + bw && row >= by && row < by + bh;
                *px = (if inside { 210 } else { base } + grain).clamp(16, 235) as u8;
            }
        }
        for row in 0..ch {
            for col in 0..cw {
                let inside = col * 2 >= bx && col * 2 < bx + bw && row * 2 >= by && row * 2 < by + bh;
                u[row * cw + col] = if inside { 90 } else { (110 + col * 40 / cw) as u8 };
                v[row * cw + col] = if inside { 170 } else { (150 - row * 40 / ch) as u8 };
            }
        }
        Yuv { w, h, y, u, v }
    }

    /// Bilinear downscale of one plane (fixed point), on one core: the part of the pipeline that stays on the CPU.
    fn scale_plane(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
        let xr = if dw > 1 { ((sw - 1) << 16) / (dw - 1) } else { 0 };
        let yr = if dh > 1 { ((sh - 1) << 16) / (dh - 1) } else { 0 };
        let xs: Vec<(usize, u64)> = (0..dw).map(|x| (((x * xr) >> 16), ((x * xr) & 0xffff) as u64)).collect();
        let mut out = Vec::with_capacity(dw * dh);
        for y in 0..dh {
            let (y0, fy) = ((y * yr) >> 16, ((y * yr) & 0xffff) as u64);
            let y1 = (y0 + 1).min(sh - 1);
            let (r0, r1) = (&src[y0 * sw..y0 * sw + sw], &src[y1 * sw..y1 * sw + sw]);
            for &(x0, fx) in &xs {
                let x1 = (x0 + 1).min(sw - 1);
                let top = u64::from(r0[x0]) * (65536 - fx) + u64::from(r0[x1]) * fx;
                let bot = u64::from(r1[x0]) * (65536 - fx) + u64::from(r1[x1]) * fx;
                out.push(((top * (65536 - fy) + bot * fy) >> 32) as u8);
            }
        }
        out
    }

    fn scale(v: &View<'_>, dw: usize, dh: usize) -> Yuv {
        let (scw, sch, dcw, dch) = (v.w.div_ceil(2), v.h.div_ceil(2), dw / 2, dh / 2);
        Yuv { w: dw, h: dh, y: scale_plane(v.y, v.w, v.h, dw, dh), u: scale_plane(v.u, scw, sch, dcw, dch), v: scale_plane(v.v, scw, sch, dcw, dch) }
    }

    // ---- encoders ------------------------------------------------------------------------------------------------------

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum EncKind {
        Software,
        VtH264,
        VtHevc,
    }

    impl EncKind {
        fn name(self) -> &'static str {
            match self {
                EncKind::Software => "software H.264 (pure Rust)",
                EncKind::VtH264 => "VideoToolbox H.264",
                EncKind::VtHevc => "VideoToolbox HEVC",
            }
        }
    }

    enum Enc {
        Sw(Box<sw::Encoder>),
        Vt(Box<VtEncoder>),
    }

    impl Enc {
        fn new(kind: EncKind, w: usize, h: usize, kbps: u32) -> R<Enc> {
            match kind {
                EncKind::Software => {
                    let mut cfg = sw::EncoderConfig::new(w as u32, h as u32, FPS, 1);
                    cfg.profile = sw::Profile::Main;
                    cfg.bframes = 0;
                    cfg.keyint = FPS * 2;
                    cfg.rate = sw::RateControl::Vbr { target_kbps: kbps, max_kbps: kbps * 3 / 2 };
                    cfg.format = sw::PacketFormat::LengthPrefixed;
                    cfg.threads = 0;
                    Ok(Enc::Sw(Box::new(sw::Encoder::new(cfg).map_err(s)?)))
                }
                EncKind::VtH264 | EncKind::VtHevc => {
                    let profile = if kind == EncKind::VtHevc { VtProfile::HevcMain } else { VtProfile::Main };
                    let cfg = VtConfig {
                        width: w as u32,
                        height: h as u32,
                        timescale: FPS,
                        frame_duration: 1,
                        keyframe_interval: FPS * 2,
                        profile,
                        rate: VtRate::Vbr { target_kbps: kbps, max_kbps: kbps * 3 / 2 },
                    };
                    Ok(Enc::Vt(Box::new(VtEncoder::new(cfg)?)))
                }
            }
        }

        /// `Some(true)` when the live session reports a hardware encoder.
        fn hardware(&self) -> Option<bool> {
            match self {
                Enc::Sw(_) => None,
                Enc::Vt(e) => Some(e.uses_hardware()),
            }
        }

        /// Compressed bytes that came out for picture `i`.
        fn push(&mut self, i: u64, v: &View<'_>) -> R<usize> {
            match self {
                Enc::Sw(e) => {
                    let f = sw::YuvFrame { y: v.y, u: v.u, v: v.v, y_stride: v.w, uv_stride: v.w / 2 };
                    Ok(e.encode(&f, i as i64).map_err(s)?.iter().map(|p| p.data.len()).sum())
                }
                Enc::Vt(e) => Ok(e.encode(i, v.y, v.u, v.v)?.iter().map(|p| p.data.len()).sum()),
            }
        }

        fn finish(&mut self) -> R<usize> {
            match self {
                Enc::Sw(e) => Ok(e.flush().iter().map(|p| p.data.len()).sum()),
                Enc::Vt(e) => Ok(e.flush()?.iter().map(|p| p.data.len()).sum()),
            }
        }
    }

    // ---- clips ---------------------------------------------------------------------------------------------------------

    struct Source {
        path: PathBuf,
        mp4: Mp4File,
        track: usize,
        entry: SampleEntry,
        samples: Vec<Vec<u8>>,
        bytes: usize,
    }

    fn make_clip(path: &Path, w: usize, h: usize, frames: usize, kbps: u32) -> R<()> {
        let mut cfg = sw::EncoderConfig::new(w as u32, h as u32, FPS, 1);
        cfg.profile = sw::Profile::High;
        cfg.bframes = 2;
        cfg.keyint = FPS;
        cfg.scenecut = false;
        cfg.rate = sw::RateControl::Cbr { kbps };
        cfg.format = sw::PacketFormat::LengthPrefixed;
        let mut enc = sw::Encoder::new(cfg).map_err(s)?;
        let avcc = AvcConfig::parse(&enc.avcc()).map_err(s)?;
        let mut packets = Vec::new();
        for t in 0..frames {
            let f = gen_frame(w, h, t);
            let yuv = sw::YuvFrame { y: &f.y, u: &f.u, v: &f.v, y_stride: w, uv_stride: w / 2 };
            packets.extend(enc.encode(&yuv, t as i64 * STEP).map_err(s)?);
        }
        packets.extend(enc.flush());
        let first_pts = packets.iter().map(|p| p.pts).min().unwrap_or(0);
        let first_dts = packets.first().map_or(0, |p| p.dts);
        let mut wr = Mp4Writer::new(std::io::Cursor::new(Vec::new()), WriterOptions::new(Brand::Mp4)).map_err(s)?;
        let mut track = TrackConfig::new(SampleEntry::avc(avcc, w as u16, h as u16), FPS * STEP as u32);
        track.media_start = Some(first_pts - first_dts);
        let t = wr.add_track(track).map_err(s)?;
        for p in &packets {
            let sample = WriteSample { data: &p.data, duration: STEP as u32, composition_offset: (p.pts - p.dts) as i32, is_sync: p.keyframe };
            wr.write_sample(t, sample).map_err(s)?;
        }
        let bytes = wr.finish().map_err(s)?.into_inner();
        std::fs::write(path, bytes).map_err(s)
    }

    /// An HEVC clip from the same pictures, made by the VideoToolbox encoder (FilmCraft has no HEVC encoder): real HEVC
    /// Main data, so the HEVC decoders (software and hardware) have something to read.
    fn make_hevc_clip(path: &Path, w: usize, h: usize, frames: usize, kbps: u32) -> R<()> {
        let cfg = VtConfig {
            width: w as u32,
            height: h as u32,
            timescale: FPS,
            frame_duration: 1,
            keyframe_interval: FPS,
            profile: VtProfile::HevcMain,
            rate: VtRate::Vbr { target_kbps: kbps, max_kbps: kbps * 3 / 2 },
        };
        let mut enc = VtEncoder::new(cfg)?;
        let mut packets = Vec::new();
        for t in 0..frames {
            let f = gen_frame(w, h, t);
            packets.extend(enc.encode(t as u64, &f.y, &f.u, &f.v)?);
        }
        packets.extend(enc.flush()?);
        let hvcc = enc.parameter_sets().and_then(|p| p.hvcc).ok_or("the encoder gave no hvcC record")?;
        let mut wr = Mp4Writer::new(std::io::Cursor::new(Vec::new()), WriterOptions::new(Brand::Mp4)).map_err(s)?;
        let entry = SampleEntry::hevc(HevcConfig::parse(&hvcc).map_err(s)?, w as u16, h as u16);
        let t = wr.add_track(TrackConfig::new(entry, FPS)).map_err(s)?;
        for p in &packets {
            wr.write_sample(t, WriteSample { data: &p.data, duration: 1, composition_offset: 0, is_sync: p.key }).map_err(s)?;
        }
        std::fs::write(path, wr.finish().map_err(s)?.into_inner()).map_err(s)
    }

    fn open_source(path: &Path) -> R<Source> {
        let bytes = std::fs::read(path).map_err(s)?;
        let mp4 = filmcraft_isobmff::open(&bytes).map_err(s)?;
        let track = mp4.tracks.iter().position(|t| t.video().is_some()).ok_or("no video track")?;
        let t = &mp4.tracks[track];
        let entry = t.entries.first().ok_or("no sample entry")?.clone();
        let samples = (0..t.samples.len()).map(|i| mp4.read_sample(&bytes, track, i).map_err(s)).collect::<R<Vec<_>>>()?;
        Ok(Source { path: path.to_owned(), mp4, track, entry, samples, bytes: bytes.len() })
    }

    // ---- the legs ------------------------------------------------------------------------------------------------------

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Dec {
        Software,
        Hardware,
    }

    impl Dec {
        fn name(self) -> &'static str {
            match self {
                Dec::Software => "software (pure Rust)",
                Dec::Hardware => "VideoToolbox",
            }
        }
    }

    #[derive(Clone, Copy, Default, Debug)]
    struct Leg {
        frames: usize,
        decode: Stage,
        scale: Stage,
        encode: Stage,
        total: Stage,
        out_bytes: usize,
        hw_frames: u64,
        hw_sessions: u64,
        hw_declined: u64,
        hw_fallbacks: u64,
        hw_encoder: Option<bool>,
    }

    /// Decode the whole clip, optionally scale each picture to `target` and encode it.
    fn pipeline(src: &Source, dec: Dec, enc: Option<(EncKind, (usize, usize), u32)>) -> R<Leg> {
        let before = hw::hw_stats();
        let mut decoder = match dec {
            Dec::Software => software_video_decoder(&src.entry),
            Dec::Hardware => make_video_decoder(&src.entry),
        }
        .map_err(s)?;
        let mut encoder = match enc {
            Some((kind, (w, h), kbps)) => Some(Enc::new(kind, w, h, kbps)?),
            None => None,
        };
        let mut leg = Leg { hw_encoder: encoder.as_ref().and_then(Enc::hardware), ..Leg::default() };
        let whole = tick();
        let mut handle = |frames: Vec<DecodedFrame>, leg: &mut Leg| -> R<()> {
            for f in frames {
                leg.frames += 1;
                let (Some(e), Some((_, (tw, th), _))) = (encoder.as_mut(), enc) else { continue };
                let v = view_of(&f)?;
                let t = tick();
                let scaled = if (v.w, v.h) == (tw, th) { None } else { Some(scale(&v, tw, th)) };
                leg.scale.add(tock(t));
                let t = tick();
                leg.out_bytes += match &scaled {
                    Some(sc) => e.push(leg.frames as u64 - 1, &sc.view())?,
                    None => e.push(leg.frames as u64 - 1, &v)?,
                };
                leg.encode.add(tock(t));
            }
            Ok(())
        };
        for (i, data) in src.samples.iter().enumerate() {
            let pts = src.mp4.tracks[src.track].samples[i].pts;
            let t = tick();
            let frames = decoder.decode(data, pts).map_err(s)?;
            leg.decode.add(tock(t));
            handle(frames, &mut leg)?;
        }
        let t = tick();
        let frames = decoder.flush();
        leg.decode.add(tock(t));
        handle(frames, &mut leg)?;
        if let Some(e) = encoder.as_mut() {
            let t = tick();
            leg.out_bytes += e.finish()?;
            leg.encode.add(tock(t));
        }
        leg.total = tock(whole);
        let after = hw::hw_stats();
        leg.hw_frames = after.frames - before.frames;
        leg.hw_sessions = after.sessions - before.sessions;
        leg.hw_declined = after.declined - before.declined;
        leg.hw_fallbacks = after.fallbacks - before.fallbacks;
        if leg.frames != src.samples.len() {
            return Err(format!("{} pictures out of {} samples", leg.frames, src.samples.len()));
        }
        Ok(leg)
    }

    /// Encode ready-made pictures (generated at the target size, so no decode and no scale in the measurement).
    fn encode_only(frames: &[Yuv], kind: EncKind, kbps: u32) -> R<Leg> {
        let first = frames.first().ok_or("no pictures")?;
        let mut e = Enc::new(kind, first.w, first.h, kbps)?;
        let mut leg = Leg { hw_encoder: e.hardware(), ..Leg::default() };
        let whole = tick();
        for (i, f) in frames.iter().enumerate() {
            let t = tick();
            leg.out_bytes += e.push(i as u64, &f.view())?;
            leg.encode.add(tock(t));
            leg.frames += 1;
        }
        let t = tick();
        leg.out_bytes += e.finish()?;
        leg.encode.add(tock(t));
        leg.total = tock(whole);
        Ok(leg)
    }

    fn reps<T: Copy>(n: usize, mut f: impl FnMut() -> R<T>) -> R<Vec<T>> {
        (0..n).map(|_| f()).collect()
    }

    // ---- reporting -----------------------------------------------------------------------------------------------------

    /// The run of `v` with the median total CPU time (CPU time is steadier than the wall clock under load).
    fn pick(v: &[Leg]) -> R<Leg> {
        median(v, |l| ms(l.total.cpu)).ok_or_else(|| "no runs".to_string())
    }

    fn best_wall(v: &[Leg]) -> f64 {
        v.iter().map(|l| ms(l.total.wall)).fold(f64::INFINITY, f64::min)
    }

    fn cmd(program: &str, args: &[&str]) -> String {
        Command::new(program).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
    }

    fn per_frame(d: Duration, frames: usize) -> f64 {
        ms(d) / frames.max(1) as f64
    }

    struct Args {
        sizes: Vec<(usize, usize)>,
        n_reps: usize,
        dir: PathBuf,
    }

    fn parse_args() -> R<Args> {
        let mut sizes = vec![(1080usize, 120usize), (2160, 60)];
        let mut n_reps = 3;
        let mut dir = std::env::temp_dir().join("lightcraft-hw-bench");
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            let mut val = || it.next().ok_or_else(|| format!("{a} needs a value"));
            match a.as_str() {
                "--frames" => {
                    sizes = val()?
                        .split(',')
                        .map(|p| {
                            let (h, n) = p.split_once('=').ok_or("--frames wants 1080=120,2160=60")?;
                            Ok((h.parse().map_err(s)?, n.parse().map_err(s)?))
                        })
                        .collect::<R<Vec<_>>>()?;
                }
                "--reps" => n_reps = val()?.parse().map_err(s)?,
                "--dir" => dir = PathBuf::from(val()?),
                other => return Err(format!("unknown argument {other}")),
            }
        }
        Ok(Args { sizes, n_reps, dir })
    }

    pub fn run() -> R<()> {
        let args = parse_args()?;
        std::fs::create_dir_all(&args.dir).map_err(s)?;
        let registered = filmcraft_platform::register();
        println!("# Hardware video bench\n");
        println!(
            "- machine: {} · macOS {} · {} cores",
            cmd("sysctl", &["-n", "machdep.cpu.brand_string"]),
            cmd("sw_vers", &["-productVersion"]),
            cmd("sysctl", &["-n", "hw.ncpu"])
        );
        println!("- load average at start: {}", cmd("sh", &["-c", "uptime | sed 's/.*load averages*: //'"]));
        println!("- reps per row: {} (wall = best, CPU = median); clips {} fps, High profile, 2 B-frames, key frame every second", args.n_reps, FPS);
        println!("- `register()`: {registered:?} · decoder backend: {:?}", hw::hw_backend());
        if !matches!(registered, Availability::Available(_)) {
            println!("\nskipped: no hardware backend on this machine");
            return Ok(());
        }
        let mut rows = Rows::default();
        for &(h, frames) in &args.sizes {
            let w = h * 16 / 9;
            let kbps_src = if h >= 2160 { 40_000 } else { 12_000 };
            let path = args.dir.join(format!("clip-{h}p-{frames}.mp4"));
            if !path.exists() {
                eprintln!("making {} ({frames} pictures at {w}x{h}) …", path.display());
                make_clip(&path, w, h, frames, kbps_src)?;
            }
            let src = open_source(&path)?;
            // "lighter copy" targets: 2160p to 1080p, 1080p to 720p (Balanced)
            let (th, kbps) = if h >= 2160 { (1080usize, 5000u32) } else { (720, 2500) };
            let tw = th * 16 / 9 / 2 * 2;
            let label = format!("{h}p H.264, {} pictures, {:.1} MB", src.samples.len(), src.bytes as f64 / 1e6);
            println!("\n## {label}\n");
            let session = hw::hw_stats();
            let hw_ok = filmcraft_platform::hardware_decoder_for(&src.entry);
            println!(
                "- `hardware_decoder_for(stream)`: **{hw_ok}** (a VideoToolbox session {})",
                if hw_ok { "opens for this stream" } else { "does not open" }
            );
            let _ = session;

            // decode only
            let mut table =
                Table::new(&["decoder", "wall ms/pic", "CPU ms/pic", "pics/s (wall)", "CPU ÷ wall", "hw pictures", "declined", "fallbacks"]);
            for d in [Dec::Software, Dec::Hardware] {
                let runs = reps(args.n_reps, || pipeline(&src, d, None))?;
                let l = pick(&runs)?;
                let wall = best_wall(&runs);
                table.row(vec![
                    d.name().into(),
                    format!("{:.2}", wall / l.frames as f64),
                    format!("{:.2}", per_frame(l.total.cpu, l.frames)),
                    format!("{:.1}", l.frames as f64 / (wall / 1000.0)),
                    format!("{:.2}", ms(l.total.cpu) / wall),
                    format!("{}", l.hw_frames),
                    format!("{}", l.hw_declined),
                    format!("{}", l.hw_fallbacks),
                ]);
                rows.decode.push((h, d, wall / l.frames as f64, per_frame(l.total.cpu, l.frames), l.hw_frames));
            }
            println!("\n**Decode only** ({w}x{h} to pictures in memory)\n\n{}", table.render());

            // encode only
            let pics: Vec<Yuv> = (0..src.samples.len().min(60)).map(|t| gen_frame(tw, th, t)).collect();
            let mut table = Table::new(&["encoder", "wall ms/pic", "CPU ms/pic", "pics/s (wall)", "CPU ÷ wall", "hardware flag", "output kbit/s"]);
            for k in [EncKind::Software, EncKind::VtH264, EncKind::VtHevc] {
                let runs = reps(args.n_reps, || encode_only(&pics, k, kbps))?;
                let l = pick(&runs)?;
                let wall = best_wall(&runs);
                let kbit = l.out_bytes as f64 * 8.0 / 1000.0 / (l.frames as f64 / f64::from(FPS));
                table.row(vec![
                    k.name().into(),
                    format!("{:.2}", wall / l.frames as f64),
                    format!("{:.2}", per_frame(l.total.cpu, l.frames)),
                    format!("{:.1}", l.frames as f64 / (wall / 1000.0)),
                    format!("{:.2}", ms(l.total.cpu) / wall),
                    l.hw_encoder.map_or("n/a (CPU)".into(), |b| format!("UsingHardwareAcceleratedVideoEncoder = {b}")),
                    format!("{kbit:.0} (asked {kbps})"),
                ]);
                rows.encode.push((th, k, wall / l.frames as f64, per_frame(l.total.cpu, l.frames)));
            }
            println!("\n**Encode only** ({tw}x{th}, {} generated pictures)\n\n{}", pics.len(), table.render());

            // the lighter-copy pipeline
            let mut table =
                Table::new(&["path", "wall ms/pic", "CPU ms/pic", "pics/s (wall)", "decode CPU", "scale CPU", "encode CPU", "CPU ÷ wall"]);
            let paths = [
                ("A software decode + software encode", Dec::Software, EncKind::Software),
                ("B VideoToolbox decode + software encode", Dec::Hardware, EncKind::Software),
                ("C VideoToolbox decode + VideoToolbox encode", Dec::Hardware, EncKind::VtH264),
                ("D software decode + VideoToolbox encode", Dec::Software, EncKind::VtH264),
            ];
            for (name, d, k) in paths {
                let runs = reps(args.n_reps, || pipeline(&src, d, Some((k, (tw, th), kbps))))?;
                let l = pick(&runs)?;
                let wall = best_wall(&runs);
                let n = l.frames as f64;
                table.row(vec![
                    name.into(),
                    format!("{:.2}", wall / n),
                    format!("{:.2}", per_frame(l.total.cpu, l.frames)),
                    format!("{:.1}", n / (wall / 1000.0)),
                    format!("{:.2}", per_frame(l.decode.cpu, l.frames)),
                    format!("{:.2}", per_frame(l.scale.cpu, l.frames)),
                    format!("{:.2}", per_frame(l.encode.cpu, l.frames)),
                    format!("{:.2}", ms(l.total.cpu) / wall),
                ]);
                rows.pipeline.push((h, name, wall / n, per_frame(l.total.cpu, l.frames)));
            }
            println!("\n**Lighter copy: decode, scale to {tw}x{th} on the CPU, encode at {kbps} kbit/s**\n\n{}", table.render());

            // a poster: one key frame
            poster(&src, args.n_reps)?;

            // the same pictures as HEVC (what an iPhone records), decoded in software and in hardware
            let hevc_path = args.dir.join(format!("clip-{h}p-{frames}-hevc.mp4"));
            if !hevc_path.exists() {
                make_hevc_clip(&hevc_path, w, h, frames, kbps_src)?;
            }
            let hevc = open_source(&hevc_path)?;
            println!("\n## {h}p HEVC (VideoToolbox-encoded), {} pictures, {:.1} MB\n", hevc.samples.len(), hevc.bytes as f64 / 1e6);
            println!("- `hardware_decoder_for(stream)`: **{}**", filmcraft_platform::hardware_decoder_for(&hevc.entry));
            let mut table =
                Table::new(&["decoder", "wall ms/pic", "CPU ms/pic", "pics/s (wall)", "CPU ÷ wall", "hw pictures", "declined", "fallbacks"]);
            for d in [Dec::Software, Dec::Hardware] {
                let runs = reps(args.n_reps, || pipeline(&hevc, d, None))?;
                let l = pick(&runs)?;
                let wall = best_wall(&runs);
                table.row(vec![
                    d.name().into(),
                    format!("{:.2}", wall / l.frames as f64),
                    format!("{:.2}", per_frame(l.total.cpu, l.frames)),
                    format!("{:.1}", l.frames as f64 / (wall / 1000.0)),
                    format!("{:.2}", ms(l.total.cpu) / wall),
                    format!("{}", l.hw_frames),
                    format!("{}", l.hw_declined),
                    format!("{}", l.hw_fallbacks),
                ]);
                rows.hevc.push((h, d, wall / l.frames as f64, per_frame(l.total.cpu, l.frames)));
            }
            println!("\n**HEVC decode only**\n\n{}", table.render());
            poster(&hevc, args.n_reps)?;
        }
        println!("\n## Summary\n\n{}", rows.summary());
        Ok(())
    }

    /// A poster is one key frame: what does it cost to decode it in software and in hardware, session creation included?
    fn poster(src: &Source, n: usize) -> R<()> {
        let t = &src.mp4.tracks[src.track];
        let at = t.sample_at_pts(t.duration as i64 / 10).unwrap_or(0);
        let key = t.sync_sample_before(at);
        let data = src.samples.get(key).ok_or("no key frame")?;
        let pts = t.samples.get(key).map_or(0, |s| s.pts);
        let one = |mk: &dyn Fn() -> filmcraft_codecs::Result<Box<dyn filmcraft_codecs::VideoDecoder>>, reuse: bool| -> R<Stage> {
            let all = tick();
            let mut d = mk().map_err(s)?;
            let mut got = d.decode(data, pts).map_err(s)?;
            got.extend(d.flush());
            if got.is_empty() {
                return Err("no picture".into());
            }
            if reuse {
                // a second poster from the same session: reset, decode again
                d.reset();
                let t = tick();
                let mut again = d.decode(data, pts).map_err(s)?;
                again.extend(d.flush());
                if again.is_empty() {
                    return Err("no picture on reuse".into());
                }
                return Ok(tock(t));
            }
            Ok(tock(all))
        };
        let stage = |v: Vec<Stage>| median(&v, |s| ms(s.wall)).unwrap_or_default();
        let mk_sw = || software_video_decoder(&src.entry);
        let mk_hw = || make_video_decoder(&src.entry);
        let reps10 = n.max(10);
        let sw_dec = stage(reps(reps10, || one(&mk_sw, false))?);
        let hw_cold = stage(reps(reps10, || one(&mk_hw, false))?);
        let hw_warm = stage(reps(reps10, || one(&mk_hw, true))?);
        let product = stage(reps(reps10, || {
            let all = tick();
            let clip = lightcraft_video::Clip::open_path(&src.path).map_err(s)?;
            clip.poster(512).map_err(s)?;
            Ok(tock(all))
        })?);
        let mut table = Table::new(&["poster (one key frame)", "wall ms", "CPU ms"]);
        for (name, st) in [
            ("software decode only", sw_dec),
            ("VideoToolbox decode, new session each time", hw_cold),
            ("VideoToolbox decode, session already open", hw_warm),
            ("`lightcraft_video::Clip::poster(512)` today (open + decode + colour + resize)", product),
        ] {
            table.row(vec![name.into(), format!("{:.1}", ms(st.wall)), format!("{:.1}", ms(st.cpu))]);
        }
        println!("\n**Poster frame** (median of {reps10})\n\n{}", table.render());
        Ok(())
    }

    // ---- tables --------------------------------------------------------------------------------------------------------

    struct Table {
        head: Vec<String>,
        rows: Vec<Vec<String>>,
    }

    impl Table {
        fn new(head: &[&str]) -> Table {
            Table { head: head.iter().map(|h| (*h).to_string()).collect(), rows: Vec::new() }
        }
        fn row(&mut self, r: Vec<String>) {
            self.rows.push(r);
        }
        fn render(&self) -> String {
            let line = |c: &[String]| format!("| {} |", c.join(" | "));
            let mut out = vec![line(&self.head), format!("|{}|", vec!["---"; self.head.len()].join("|"))];
            out.extend(self.rows.iter().map(|r| line(r)));
            out.join("\n")
        }
    }

    #[derive(Default)]
    struct Rows {
        decode: Vec<(usize, Dec, f64, f64, u64)>,
        encode: Vec<(usize, EncKind, f64, f64)>,
        pipeline: Vec<(usize, &'static str, f64, f64)>,
        hevc: Vec<(usize, Dec, f64, f64)>,
    }

    impl Rows {
        /// Ratios the go / no-go note quotes: hardware over software, per source height.
        fn summary(&self) -> String {
            let mut out = Vec::new();
            for &(h, d, wall, cpu, _) in self.decode.iter().filter(|r| r.1 == Dec::Hardware) {
                if let Some(&(_, _, sw_wall, sw_cpu, _)) = self.decode.iter().find(|r| r.0 == h && r.1 == Dec::Software) {
                    out.push(format!(
                        "- decode {h}p {}: wall {:.1}x faster, CPU time {:.1}x lower ({:.2} vs {:.2} CPU ms per picture)",
                        d.name(),
                        sw_wall / wall,
                        sw_cpu / cpu,
                        cpu,
                        sw_cpu
                    ));
                }
            }
            for &(h, k, wall, cpu) in self.encode.iter().filter(|r| r.1 != EncKind::Software) {
                if let Some(&(_, _, sw_wall, sw_cpu)) = self.encode.iter().find(|r| r.0 == h && r.1 == EncKind::Software) {
                    out.push(format!(
                        "- encode to {h}p, {}: wall {:.1}x faster, CPU time {:.1}x lower ({:.2} vs {:.2} CPU ms per picture)",
                        k.name(),
                        sw_wall / wall,
                        sw_cpu / cpu,
                        cpu,
                        sw_cpu
                    ));
                }
            }
            for &(h, _, wall, cpu) in self.hevc.iter().filter(|r| r.1 == Dec::Hardware) {
                if let Some(&(_, _, sw_wall, sw_cpu)) = self.hevc.iter().find(|r| r.0 == h && r.1 == Dec::Software) {
                    out.push(format!(
                        "- decode {h}p HEVC VideoToolbox: wall {:.1}x faster, CPU time {:.1}x lower ({:.2} vs {:.2} CPU ms per picture)",
                        sw_wall / wall,
                        sw_cpu / cpu,
                        cpu,
                        sw_cpu
                    ));
                }
            }
            let a: Vec<_> = self.pipeline.iter().filter(|r| r.1.starts_with('A')).collect();
            for r in self.pipeline.iter().filter(|r| !r.1.starts_with('A')) {
                if let Some(a) = a.iter().find(|x| x.0 == r.0) {
                    out.push(format!("- lighter copy from {}p, {}: wall {:.1}x faster, CPU time {:.1}x lower", r.0, r.1, a.2 / r.2, a.3 / r.3));
                }
            }
            out.join("\n")
        }
    }
}
