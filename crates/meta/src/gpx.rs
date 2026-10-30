//! GPS track logs in GPX 1.0 / 1.1 (the open GPS Exchange Format, <https://www.topografix.com/gpx.asp>):
//! the timed track points of every `<trk>`, and the position at any moment for geotagging photos.
//!
//! What we use from the format: `<trk>` → `<trkseg>` → `<trkpt lat=… lon=…>` with optional `<ele>`
//! (metres) and `<time>` (an `xsd:dateTime`, UTC per the schema — a time without a zone is read as
//! UTC). Points without a time can't be matched to a photo and are left out; waypoints (`<wpt>`) and
//! routes (`<rte>`) carry no timeline and are ignored. The schema defines a track segment as one
//! continuous span of logging, so positions are only interpolated *within* a segment, never across
//! the gap where reception was lost or the logger was off.
//!
//! [`write_gpx`] is the inverse: it writes time-ordered points as a GPX 1.1 document that
//! [`parse_gpx`] reads back (and so does any other GPS tool).

use std::fmt::Write as _;

use chrono::Datelike as _;
use quick_xml::events::Event;

use crate::{DateTime, Gps};

/// Nesting / size limits (a track log is flat; these only stop hostile input).
const MAX_DEPTH: usize = 64;
const MAX_POINTS: usize = 5_000_000;

/// One timed point of a track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackPoint {
    /// Seconds since 1970-01-01T00:00:00Z (fractional when the log has sub-second times).
    pub time: f64,
    pub latitude: f64,
    pub longitude: f64,
    /// Metres above sea level.
    pub elevation: Option<f64>,
    /// Track segment number, counted across the whole file (continuous logging spans).
    pub segment: u32,
}

/// How a position was found by [`Tracklog::locate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Match {
    /// The time falls between two points of the same segment: linear interpolation.
    Interpolated,
    /// The nearest point (at a segment's start or end, or across a gap).
    Nearest,
}

/// The timed points of a GPX file, sorted by time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tracklog {
    pub points: Vec<TrackPoint>,
    /// Tracks (`<trk>`) in the file.
    pub tracks: usize,
    /// Track points without a usable time (left out of `points`).
    pub untimed: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GpxError {
    Xml(String),
    /// Not a GPX document (no `<gpx>` root).
    NotGpx,
    /// Nesting / size limits exceeded.
    TooComplex,
}

impl std::fmt::Display for GpxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpxError::Xml(e) => write!(f, "malformed GPX: {e}"),
            GpxError::NotGpx => f.write_str("not a GPX file (no <gpx> element)"),
            GpxError::TooComplex => f.write_str("GPX file too large or too deeply nested"),
        }
    }
}

impl std::error::Error for GpxError {}

/// Local name of a possibly prefixed XML name (`gpx:trkpt` → `trkpt`).
fn local(name: &[u8]) -> &[u8] {
    name.rsplit(|&b| b == b':').next().unwrap_or(name)
}

/// An `xsd:dateTime` as seconds since the Unix epoch; no zone = UTC (the GPX schema's convention).
fn parse_time(s: &str) -> Option<f64> {
    let mut d = DateTime::parse_iso(s)?;
    // a bare date (no time of day) is too coarse to place a photo
    if !s.contains('T') {
        return None;
    }
    d.offset_minutes.get_or_insert(0);
    Some(d.unix_seconds() as f64 + d.millis as f64 / 1000.0)
}

#[derive(Default)]
struct Pending {
    lat: Option<f64>,
    lon: Option<f64>,
    ele: Option<f64>,
    time: Option<f64>,
}

/// Parse a GPX document.
pub fn parse_gpx(s: &str) -> Result<Tracklog, GpxError> {
    let mut reader = quick_xml::Reader::from_str(s);
    // Mismatched end tags must be rejected; otherwise the path stack can
    // attribute a point's time or segment to the wrong enclosing element.
    reader.config_mut().check_end_names = true;
    let mut path: Vec<Vec<u8>> = Vec::new();
    let mut seen_gpx = false;
    let mut log = Tracklog::default();
    let mut segment: u32 = 0;
    let mut pt: Option<Pending> = None;
    let mut text = String::new();
    loop {
        let ev = reader.read_event().map_err(|e| GpxError::Xml(e.to_string()))?;
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let name = local(e.name().as_ref()).to_vec();
                let parent = path.last().map(Vec::as_slice);
                match name.as_slice() {
                    b"gpx" if path.is_empty() => seen_gpx = true,
                    b"trk" if parent == Some(b"gpx") => log.tracks += 1,
                    b"trkseg" if parent == Some(b"trk") && log.points.last().is_some_and(|p| p.segment == segment) => segment += 1,
                    b"trkpt" if parent == Some(b"trkseg") => {
                        let mut p = Pending::default();
                        for a in e.attributes().with_checks(false).flatten() {
                            let v = String::from_utf8_lossy(&a.value);
                            match local(a.key.as_ref()) {
                                b"lat" => p.lat = v.trim().parse().ok(),
                                b"lon" => p.lon = v.trim().parse().ok(),
                                _ => {}
                            }
                        }
                        pt = Some(p);
                    }
                    _ => {}
                }
                text.clear();
                if matches!(ev, Event::Start(_)) {
                    if path.len() >= MAX_DEPTH {
                        return Err(GpxError::TooComplex);
                    }
                    path.push(name);
                } else if name == b"trkpt" {
                    finish_point(&mut log, pt.take(), segment)?;
                }
            }
            Event::Text(t) => text.push_str(&t.decode().map_err(|e| GpxError::Xml(e.to_string()))?),
            Event::CData(t) => text.push_str(&String::from_utf8_lossy(&t)),
            Event::End(_) => {
                let Some(name) = path.pop() else { continue };
                let parent = path.last().map(Vec::as_slice);
                match name.as_slice() {
                    b"ele" if parent == Some(b"trkpt") => {
                        if let Some(p) = pt.as_mut() {
                            p.ele = text.trim().parse().ok().filter(|v: &f64| v.is_finite());
                        }
                    }
                    b"time" if parent == Some(b"trkpt") => {
                        if let Some(p) = pt.as_mut() {
                            p.time = parse_time(text.trim());
                        }
                    }
                    b"trkpt" => finish_point(&mut log, pt.take(), segment)?,
                    _ => {}
                }
                text.clear();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !seen_gpx {
        return Err(GpxError::NotGpx);
    }
    if !path.is_empty() {
        return Err(GpxError::Xml("unclosed GPX elements".into()));
    }
    // stable: points logged at the same second keep their file order
    log.points.sort_by(|a, b| a.time.total_cmp(&b.time));
    Ok(log)
}

fn finish_point(log: &mut Tracklog, p: Option<Pending>, segment: u32) -> Result<(), GpxError> {
    let Some(p) = p else { return Ok(()) };
    let (Some(lat), Some(lon)) = (p.lat, p.lon) else { return Ok(()) };
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return Ok(());
    }
    let Some(time) = p.time else {
        log.untimed += 1;
        return Ok(());
    };
    if log.points.len() >= MAX_POINTS {
        return Err(GpxError::TooComplex);
    }
    log.points.push(TrackPoint { time, latitude: lat, longitude: lon, elevation: p.ele, segment });
    Ok(())
}

/// Longest track name [`write_gpx`] keeps, in characters.
const MAX_NAME_CHARS: usize = 500;

/// GPX 1.1 text for `points` (time-ordered), the inverse of [`parse_gpx`]: one `<trk>` named `name`,
/// one `<trkseg>` per run of equal `segment`, each point with its position (6 decimals), `<ele>`
/// (1 decimal, left out when not finite) and `<time>` (UTC, with milliseconds only when the time has
/// them).
///
/// Points that are not valid are skipped, never written half: a non-finite time, latitude or
/// longitude, `|lat| > 90`, `|lon| > 180`, or a time outside the years 1 to 9999 (the span of an
/// `xsd:dateTime` every GPX reader takes). Skipped points do not start or end a segment. The name
/// loses its control characters and is cut to 500 characters; a name left empty is not written. The
/// points are written in the order given (the caller sorts them); no input panics, and an empty or
/// all-invalid list gives a valid document with no `<trkseg>`.
pub fn write_gpx(name: Option<&str>, points: &[TrackPoint]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gpx version=\"1.1\" creator=\"LightCraft\" xmlns=\"http://www.topografix.com/GPX/1/1\">\n<trk>\n",
    );
    if let Some(name) = name.map(clean_name).filter(|n| !n.is_empty()) {
        out.push_str("<name>");
        out.push_str(&name);
        out.push_str("</name>\n");
    }
    // the `segment` of the `<trkseg>` being written, if one is open
    let mut open: Option<u32> = None;
    for p in points {
        // NaN and infinity fail these comparisons too
        if !(p.latitude.abs() <= 90.0 && p.longitude.abs() <= 180.0) {
            continue;
        }
        let Some((when, has_millis)) = utc(p.time) else { continue };
        if open != Some(p.segment) {
            if open.is_some() {
                out.push_str("</trkseg>\n");
            }
            out.push_str("<trkseg>\n");
            open = Some(p.segment);
        }
        // writing to a String cannot fail
        let _ = write!(out, "<trkpt lat=\"{:.6}\" lon=\"{:.6}\">", p.latitude, p.longitude);
        if let Some(ele) = p.elevation.filter(|e| e.is_finite()) {
            let _ = write!(out, "<ele>{ele:.1}</ele>");
        }
        let layout = if has_millis { "%Y-%m-%dT%H:%M:%S%.3fZ" } else { "%Y-%m-%dT%H:%M:%SZ" };
        let _ = writeln!(out, "<time>{}</time></trkpt>", when.format(layout));
    }
    if open.is_some() {
        out.push_str("</trkseg>\n");
    }
    out.push_str("</trk>\n</gpx>\n");
    out
}

/// `name` made safe as XML text: control characters (and the two non-characters XML forbids)
/// dropped, at most [`MAX_NAME_CHARS`] characters kept, `& < > " '` escaped.
fn clean_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().filter(|c| !c.is_control() && !matches!(c, '\u{fffe}' | '\u{ffff}')).take(MAX_NAME_CHARS) {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// `t` seconds since the epoch as a UTC date-time rounded to the millisecond, and whether that has a
/// millisecond part; `None` when `t` is not finite or the year is not in 1..=9999.
fn utc(t: f64) -> Option<(chrono::DateTime<chrono::Utc>, bool)> {
    if !t.is_finite() {
        return None;
    }
    // the cast saturates, so a huge `t` ends up as a year `from_timestamp` or the range check refuses
    let millis = (t * 1000.0).round() as i64;
    let nanos = u32::try_from(millis.rem_euclid(1000)).ok()? * 1_000_000;
    let when = chrono::DateTime::from_timestamp(millis.div_euclid(1000), nanos)?;
    (1..=9999).contains(&when.year()).then_some((when, nanos != 0))
}

impl Tracklog {
    /// First and last point times (seconds since the epoch, UTC).
    pub fn span(&self) -> Option<(f64, f64)> {
        Some((self.points.first()?.time, self.points.last()?.time))
    }

    /// Where the logger was at `time` (seconds since the epoch, UTC).
    ///
    /// Between two points of the same segment that are at most `max_gap` seconds apart, the position
    /// is interpolated linearly in time. Otherwise the nearest point counts if it is within `max_gap`
    /// seconds; beyond that there is no position (the photo was taken while nothing was logged).
    pub fn locate(&self, time: f64, max_gap: f64) -> Option<(Gps, Match)> {
        let pts = &self.points;
        if pts.is_empty() || !time.is_finite() {
            return None;
        }
        let i = pts.partition_point(|p| p.time < time);
        let next = pts.get(i);
        let prev = i.checked_sub(1).and_then(|j| pts.get(j));
        if let Some(n) = next
            && n.time == time
        {
            return Some((gps(n), Match::Interpolated));
        }
        if let (Some(a), Some(b)) = (prev, next)
            && a.segment == b.segment
            && b.time - a.time <= max_gap
        {
            let t = (time - a.time) / (b.time - a.time);
            return Some((lerp(a, b, t), Match::Interpolated));
        }
        let near = match (prev, next) {
            (Some(a), Some(b)) => {
                if time - a.time <= b.time - time {
                    a
                } else {
                    b
                }
            }
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => return None,
        };
        ((near.time - time).abs() <= max_gap).then(|| (gps(near), Match::Nearest))
    }
}

fn gps(p: &TrackPoint) -> Gps {
    Gps { latitude: p.latitude, longitude: p.longitude, altitude: p.elevation }
}

/// Linear interpolation, taking the short way across the ±180° meridian.
fn lerp(a: &TrackPoint, b: &TrackPoint, t: f64) -> Gps {
    let mut dlon = b.longitude - a.longitude;
    if dlon > 180.0 {
        dlon -= 360.0;
    } else if dlon < -180.0 {
        dlon += 360.0;
    }
    let mut lon = a.longitude + dlon * t;
    if lon > 180.0 {
        lon -= 360.0;
    } else if lon < -180.0 {
        lon += 360.0;
    }
    let altitude = match (a.elevation, b.elevation) {
        (Some(x), Some(y)) => Some(x + (y - x) * t),
        (x, y) => x.or(y),
    };
    Gps { latitude: a.latitude + (b.latitude - a.latitude) * t, longitude: lon, altitude }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="test" xmlns="http://www.topografix.com/GPX/1/1">
  <wpt lat="1.0" lon="1.0"><time>2026-05-01T10:00:30Z</time></wpt>
  <trk><name>Walk</name>
    <trkseg>
      <trkpt lat="46.0000" lon="7.0000"><ele>1600</ele><time>2026-05-01T10:00:00Z</time></trkpt>
      <trkpt lat="46.0010" lon="7.0020"><ele>1610</ele><time>2026-05-01T10:01:00Z</time></trkpt>
      <trkpt lat="46.0020" lon="7.0040"><time>2026-05-01T10:02:00.500Z</time></trkpt>
      <trkpt lat="46.5" lon="7.5"/>
    </trkseg>
    <trkseg>
      <trkpt lat="47.0000" lon="8.0000"><time>2026-05-01T12:00:00Z</time></trkpt>
      <trkpt lat="47.0010" lon="8.0010"><time>2026-05-01T12:00:10+00:00</time></trkpt>
    </trkseg>
  </trk>
</gpx>"#;

    fn t(s: &str) -> f64 {
        parse_time(s).unwrap()
    }

    #[test]
    fn parses_timed_track_points() {
        let log = parse_gpx(LOG).unwrap();
        assert_eq!(log.tracks, 1);
        assert_eq!(log.points.len(), 5, "waypoints and the untimed point are left out");
        assert_eq!(log.untimed, 1);
        assert_eq!(log.points[0].elevation, Some(1600.0));
        assert_eq!(log.points[2].time, t("2026-05-01T10:02:00Z") + 0.5);
        assert_eq!((log.points[0].segment, log.points[2].segment, log.points[3].segment), (0, 0, 1));
        assert_eq!(log.span(), Some((t("2026-05-01T10:00:00Z"), t("2026-05-01T12:00:10Z"))));
        // 2026-05-01T10:00:00Z
        assert_eq!(log.points[0].time, 1_777_629_600.0);
    }

    #[test]
    fn interpolates_within_a_segment() {
        let log = parse_gpx(LOG).unwrap();
        let (g, m) = log.locate(t("2026-05-01T10:00:30Z"), 300.0).unwrap();
        assert_eq!(m, Match::Interpolated);
        assert!((g.latitude - 46.0005).abs() < 1e-9 && (g.longitude - 7.0010).abs() < 1e-9, "{g:?}");
        assert_eq!(g.altitude, Some(1605.0));
        // exactly on a point
        let (g, _) = log.locate(t("2026-05-01T10:01:00Z"), 300.0).unwrap();
        assert_eq!((g.latitude, g.longitude), (46.0010, 7.0020));
    }

    #[test]
    fn never_interpolates_across_segments() {
        let log = parse_gpx(LOG).unwrap();
        // between the segments (10:02 … 12:00): only near either end
        let (g, m) = log.locate(t("2026-05-01T10:04:00Z"), 300.0).unwrap();
        assert_eq!((m, g.latitude), (Match::Nearest, 46.0020));
        let (g, m) = log.locate(t("2026-05-01T11:58:00Z"), 300.0).unwrap();
        assert_eq!((m, g.latitude), (Match::Nearest, 47.0));
        assert!(log.locate(t("2026-05-01T11:00:00Z"), 300.0).is_none(), "an hour from any point");
        // before the first and after the last point
        assert!(log.locate(t("2026-05-01T09:59:00Z"), 300.0).is_some());
        assert!(log.locate(t("2026-05-01T09:50:00Z"), 300.0).is_none());
        assert!(log.locate(t("2026-05-01T12:10:00Z"), 300.0).is_none());
    }

    #[test]
    fn max_gap_limits_interpolation() {
        let log = parse_gpx(LOG).unwrap();
        // the points are 60 s apart: with a 20 s limit, only the nearest point within 20 s counts
        assert!(log.locate(t("2026-05-01T10:00:30Z"), 20.0).is_none());
        let (g, m) = log.locate(t("2026-05-01T10:00:50Z"), 20.0).unwrap();
        assert_eq!((m, g.latitude), (Match::Nearest, 46.0010));
    }

    #[test]
    fn gpx_1_0_prefixes_and_zones() {
        let s = r#"<gpx:gpx version="1.0" xmlns:gpx="http://www.topografix.com/GPX/1/0"><gpx:trk><gpx:trkseg>
            <gpx:trkpt lat="-33.9" lon="151.2"><gpx:time>2026-01-02T10:00:00+11:00</gpx:time></gpx:trkpt>
            <gpx:trkpt lat="-33.8" lon="151.3"><gpx:time>2026-01-01T23:01:00</gpx:time></gpx:trkpt>
        </gpx:trkseg></gpx:trk></gpx:gpx>"#;
        let log = parse_gpx(s).unwrap();
        assert_eq!(log.points.len(), 2);
        // +11:00 is converted to UTC; no zone = UTC
        assert_eq!(log.points[1].time - log.points[0].time, 60.0);
    }

    #[test]
    fn non_finite_elevation_is_dropped() {
        for ele in ["inf", "-inf", "NaN", "infinity"] {
            let s = format!(
                r#"<gpx><trk><trkseg><trkpt lat="45" lon="7"><ele>{ele}</ele><time>2026-01-01T00:00:00Z</time></trkpt></trkseg></trk></gpx>"#
            );
            let log = parse_gpx(&s).unwrap();
            assert_eq!(log.points.len(), 1, "{ele}");
            assert_eq!(log.points[0].elevation, None, "{ele}");
        }
    }

    #[test]
    fn interpolation_across_the_antimeridian() {
        let a = TrackPoint { time: 0.0, latitude: 0.0, longitude: 179.0, elevation: None, segment: 0 };
        let b = TrackPoint { time: 10.0, latitude: 0.0, longitude: -179.0, elevation: None, segment: 0 };
        let g = lerp(&a, &b, 0.75);
        assert!((g.longitude - -179.5).abs() < 1e-9, "{g:?}");
    }

    #[test]
    fn rejects_non_gpx_and_bad_input() {
        assert_eq!(parse_gpx("<kml><Document/></kml>"), Err(GpxError::NotGpx));
        assert!(parse_gpx("").is_err());
        // A partial logger write must not be mistaken for a complete track,
        // even if it contains a valid timed point.
        let point = r#"<gpx><trk><trkseg><trkpt lat="45" lon="7"><time>2026-01-01T00:00:00Z</time></trkpt></trkseg></trk>"#;
        assert!(matches!(parse_gpx(point), Err(GpxError::Xml(_))));
        let wrong_close = r#"<gpx><trk><trkseg><trkpt lat="45" lon="7"><time>2026-01-01T00:00:00Z</time></trkpt></trk></trkseg></gpx>"#;
        assert!(matches!(parse_gpx(wrong_close), Err(GpxError::Xml(_))));
        assert!(
            parse_gpx("<gpx><trk><trkseg><trkpt lat=\"x\" lon=\"1\"><time>2026-01-01T00:00:00Z</time></trkpt></trkseg></trk></gpx>")
                .unwrap()
                .points
                .is_empty()
        );
        let deep = "<gpx>".to_string() + &"<a>".repeat(100);
        assert_eq!(parse_gpx(&deep), Err(GpxError::TooComplex));
    }

    // ---- write_gpx ----

    const BASE: f64 = 1_777_629_600.0; // 2026-05-01T10:00:00Z

    fn pt(time: f64, latitude: f64, longitude: f64, elevation: Option<f64>, segment: u32) -> TrackPoint {
        TrackPoint { time, latitude, longitude, elevation, segment }
    }

    /// Writes `pts`, reads the text back with [`parse_gpx`], and checks every point to the precision
    /// the writer promises (1e-6 degrees, half a millisecond, 0.05 m) and the segment structure (each
    /// run of equal `segment` is one segment; the parser numbers them 0, 1, 2, ...). Returns the text.
    fn assert_round_trip(pts: &[TrackPoint]) -> String {
        let doc = write_gpx(Some("t"), pts);
        let log = parse_gpx(&doc).unwrap_or_else(|e| panic!("{e}: {doc}"));
        assert_eq!((log.tracks, log.untimed, log.points.len()), (1, 0, pts.len()), "{doc}");
        let mut run = 0;
        for (i, (a, b)) in pts.iter().zip(&log.points).enumerate() {
            if i > 0 && pts[i - 1].segment != a.segment {
                run += 1;
            }
            assert!((a.latitude - b.latitude).abs() <= 1e-6, "point {i} lat: {a:?} vs {b:?}");
            assert!((a.longitude - b.longitude).abs() <= 1e-6, "point {i} lon: {a:?} vs {b:?}");
            assert!((a.time - b.time).abs() <= 0.5e-3 + 1e-6, "point {i} time: {a:?} vs {b:?}");
            match (a.elevation, b.elevation) {
                (None, None) => {}
                (Some(x), Some(y)) => assert!((x - y).abs() <= 0.05 + 1e-9, "point {i} ele: {a:?} vs {b:?}"),
                (x, y) => panic!("point {i} ele {x:?} vs {y:?}"),
            }
            assert_eq!(b.segment, run, "point {i} segment");
        }
        doc
    }

    /// The text between `open` and `close` in `doc`.
    fn between<'a>(doc: &'a str, open: &str, close: &str) -> Option<&'a str> {
        let start = doc.find(open)? + open.len();
        Some(&doc[start..start + doc[start..].find(close)?])
    }

    fn time_text(t: f64) -> Option<String> {
        between(&write_gpx(None, &[pt(t, 1.0, 1.0, None, 0)]), "<time>", "</time>").map(str::to_string)
    }

    /// The `<name>` of a document with no points, unescaped; the document must be well-formed.
    fn name_text(name: Option<&str>) -> Option<String> {
        let doc = write_gpx(name, &[]);
        assert!(parse_gpx(&doc).is_ok(), "{doc}");
        let raw = between(&doc, "<name>", "</name>")?;
        Some(quick_xml::escape::unescape(raw).unwrap().into_owned())
    }

    #[test]
    fn writes_the_documented_text() {
        let pts = [pt(BASE, -33.9, 151.2, Some(12.34), 0), pt(BASE + 0.5, 46.0, -7.0, None, 0)];
        assert_eq!(
            write_gpx(Some("Walk"), &pts),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <gpx version=\"1.1\" creator=\"LightCraft\" xmlns=\"http://www.topografix.com/GPX/1/1\">\n\
             <trk>\n<name>Walk</name>\n<trkseg>\n\
             <trkpt lat=\"-33.900000\" lon=\"151.200000\"><ele>12.3</ele><time>2026-05-01T10:00:00Z</time></trkpt>\n\
             <trkpt lat=\"46.000000\" lon=\"-7.000000\"><time>2026-05-01T10:00:00.500Z</time></trkpt>\n\
             </trkseg>\n</trk>\n</gpx>\n"
        );
    }

    #[test]
    fn points_and_segments_round_trip() {
        let pts = [
            pt(BASE, 46.0, 7.0, Some(1600.0), 0),
            pt(BASE + 1.0, 46.001, 7.002, Some(-12.34), 0),
            pt(BASE + 2.5, -33.868820, 151.209296, None, 0),
            pt(BASE + 60.0, 40.7128, -74.006, Some(10.04), 1),
            pt(BASE + 61.123, -0.000001, -179.999999, None, 1),
            pt(BASE + 3600.0, 90.0, 180.0, Some(0.0), 7),
            pt(BASE + 3601.0, -90.0, -180.0, None, 3),
            pt(BASE + 3602.0, 1.0, 1.0, None, 7),
        ];
        // runs [0 0 0] [1 1] [7] [3] [7]: a number seen before still starts a new <trkseg>
        let doc = assert_round_trip(&pts);
        assert_eq!(doc.matches("<trkseg>").count(), 5);
        assert_eq!(doc.matches("</trkseg>").count(), 5);
        assert_eq!(doc.matches("<trkpt ").count(), 8);
        assert_eq!(doc.matches("<ele>").count(), 4);
    }

    #[test]
    fn a_log_read_then_written_reads_back_the_same() {
        let first = parse_gpx(LOG).unwrap();
        let second = parse_gpx(&write_gpx(Some("Walk"), &first.points)).unwrap();
        assert_eq!(second.points, first.points);
        assert_eq!(second.tracks, 1);
    }

    #[test]
    fn times_are_utc_with_milliseconds_only_when_present() {
        assert_eq!(time_text(0.0).as_deref(), Some("1970-01-01T00:00:00Z"));
        assert_eq!(time_text(BASE).as_deref(), Some("2026-05-01T10:00:00Z"));
        assert_eq!(time_text(BASE + 0.007).as_deref(), Some("2026-05-01T10:00:00.007Z"));
        assert_eq!(time_text(BASE + 0.1).as_deref(), Some("2026-05-01T10:00:00.100Z"));
        // rounded to the nearest millisecond, carrying into the second
        assert_eq!(time_text(100.0006).as_deref(), Some("1970-01-01T00:01:40.001Z"));
        assert_eq!(time_text(100.9996).as_deref(), Some("1970-01-01T00:01:41Z"));
    }

    #[test]
    fn times_before_1970_and_far_in_the_future() {
        assert_eq!(time_text(-1.0).as_deref(), Some("1969-12-31T23:59:59Z"));
        assert_eq!(time_text(-0.25).as_deref(), Some("1969-12-31T23:59:59.750Z"));
        assert_eq!(time_text(-2_208_988_800.0).as_deref(), Some("1900-01-01T00:00:00Z"));
        assert_eq!(time_text(-62_135_596_800.0).as_deref(), Some("0001-01-01T00:00:00Z"));
        assert_eq!(time_text(253_402_300_799.999).as_deref(), Some("9999-12-31T23:59:59.999Z"));
        let pts = [
            pt(-62_135_596_800.0, 10.0, 10.0, None, 0),
            pt(-2_208_988_800.5, 11.0, 11.0, None, 0),
            pt(-0.75, 12.0, 12.0, None, 0),
            pt(4_102_444_800.0, 13.0, 13.0, None, 0),
            pt(253_402_300_799.5, 14.0, 14.0, None, 0),
        ];
        assert_round_trip(&pts);
    }

    #[test]
    fn times_outside_years_1_to_9999_are_skipped() {
        for t in [253_402_300_800.0, -62_135_596_801.0, 1e12, -1e12, 1e300, f64::MIN, f64::MAX, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(time_text(t), None, "{t}");
        }
    }

    #[test]
    fn names_are_escaped() {
        let doc = write_gpx(Some("a<b>&\"c'"), &[]);
        assert!(doc.contains("<name>a&lt;b&gt;&amp;&quot;c&apos;</name>"), "{doc}");
        assert_eq!(name_text(Some("a<b>&\"c'")).as_deref(), Some("a<b>&\"c'"));
        assert_eq!(name_text(Some("&amp;lt;")).as_deref(), Some("&amp;lt;"));
        assert_eq!(name_text(Some("Peak (1/2) - 50% [x]; ok?")).as_deref(), Some("Peak (1/2) - 50% [x]; ok?"));
    }

    #[test]
    fn names_lose_control_characters_and_keep_other_text() {
        assert_eq!(name_text(Some("a\u{0}b\u{1b}c\nd\te\rf\u{7f}g\u{85}h\u{ffff}i\u{fffe}j")).as_deref(), Some("abcdefghij"));
        let text = "Пляж, 日本の旅 🌅 e\u{301} é";
        assert_eq!(name_text(Some(text)).as_deref(), Some(text));
        assert!(write_gpx(Some(text), &[]).contains(text), "written as UTF-8, not as entities");
    }

    #[test]
    fn a_name_without_text_is_not_written() {
        for name in [None, Some(""), Some("\u{1}\u{2}\n")] {
            let doc = write_gpx(name, &[]);
            assert!(!doc.contains("<name"), "{name:?}: {doc}");
            assert!(parse_gpx(&doc).is_ok());
        }
    }

    #[test]
    fn long_names_are_cut_on_a_character_boundary() {
        for unit in ["é", "🌅", "x", "日"] {
            let name = name_text(Some(&unit.repeat(10_000))).unwrap();
            assert_eq!(name, unit.repeat(500), "{unit}");
            let name = name_text(Some(&unit.repeat(500))).unwrap();
            assert_eq!(name.chars().count(), 500, "exactly 500 stay: {unit}");
        }
        // the limit counts characters, before escaping; controls do not count
        assert_eq!(name_text(Some(&"&".repeat(600))).unwrap(), "&".repeat(500));
        assert_eq!(name_text(Some(&format!("{}{}", "\u{1}".repeat(400), "x".repeat(600)))).unwrap(), "x".repeat(500));
    }

    #[test]
    fn nothing_to_write_still_gives_a_valid_document() {
        let empty = write_gpx(None, &[]);
        assert!(!empty.contains("<trkseg"), "{empty}");
        let log = parse_gpx(&empty).unwrap();
        assert_eq!((log.tracks, log.points.len(), log.untimed), (1, 0, 0));
        assert!(empty.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gpx version=\"1.1\" creator=\"LightCraft\""));
        assert!(empty.ends_with("</trk>\n</gpx>\n"));
    }

    #[test]
    fn invalid_points_are_skipped_not_written() {
        let nan = f64::NAN;
        let inf = f64::INFINITY;
        let bad = [
            pt(nan, 1.0, 1.0, None, 0),
            pt(inf, 1.0, 1.0, None, 0),
            pt(BASE, nan, 1.0, None, 0),
            pt(BASE, inf, 1.0, None, 0),
            pt(BASE, 1.0, nan, None, 0),
            pt(BASE, 1.0, -inf, None, 0),
            pt(BASE, 90.000001, 1.0, None, 0),
            pt(BASE, -90.000001, 1.0, None, 0),
            pt(BASE, 1.0, 180.000001, None, 0),
            pt(BASE, 1.0, -180.000001, None, 0),
            pt(BASE, 1e300, 1.0, None, 0),
            pt(BASE, f64::MAX, f64::MIN, None, 0),
            pt(1e300, 1.0, 1.0, None, 0),
            pt(253_402_300_800.0, 1.0, 1.0, None, 0),
        ];
        for p in bad {
            let doc = write_gpx(Some("x"), &[p]);
            assert!(!doc.contains("<trkseg") && !doc.contains("<trkpt"), "{p:?}: {doc}");
            assert_eq!(parse_gpx(&doc).unwrap().points.len(), 0);
        }
        let doc = write_gpx(None, &bad);
        assert!(!doc.contains("<trkseg") && !doc.contains("<trkpt"), "{doc}");
    }

    #[test]
    fn an_invalid_point_neither_splits_nor_opens_a_segment() {
        let pts = [
            pt(BASE, 1.0, 1.0, None, 0),
            pt(BASE + 1.0, f64::NAN, 1.0, None, 0), // inside segment 0: no split
            pt(BASE + 2.0, 2.0, 2.0, None, 0),
            pt(BASE + 3.0, 91.0, 1.0, None, 5), // a segment of its own with nothing valid: no empty <trkseg>
            pt(BASE + 4.0, 3.0, 3.0, None, 9),
        ];
        let doc = write_gpx(None, &pts);
        assert_eq!((doc.matches("<trkseg>").count(), doc.matches("<trkpt ").count()), (2, 3), "{doc}");
        let log = parse_gpx(&doc).unwrap();
        assert_eq!(log.points.iter().map(|p| p.segment).collect::<Vec<_>>(), [0, 0, 1]);
        assert_eq!(log.points.iter().map(|p| p.latitude).collect::<Vec<_>>(), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn a_bad_elevation_drops_the_elevation_not_the_point() {
        for ele in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let doc = write_gpx(None, &[pt(BASE, 5.0, 6.0, Some(ele), 0)]);
            assert!(!doc.contains("<ele>"), "{doc}");
            assert_eq!(parse_gpx(&doc).unwrap().points.len(), 1);
        }
        // finite but absurd: still a number the reader takes back
        let log = parse_gpx(&write_gpx(None, &[pt(BASE, 5.0, 6.0, Some(1e300), 0)])).unwrap();
        assert_eq!(log.points[0].elevation, Some(1e300));
    }

    #[test]
    fn one_point_and_ties_round_trip() {
        assert_round_trip(&[pt(BASE, 12.5, -45.25, Some(3.0), 4)]);
        // equal times keep their order
        assert_round_trip(&[pt(BASE, 1.0, 1.0, None, 0), pt(BASE, 2.0, 2.0, None, 0), pt(BASE, 3.0, 3.0, None, 1)]);
    }

    #[test]
    fn a_huge_track_is_written_in_reasonable_time() {
        let n = 200_000usize;
        let pts: Vec<TrackPoint> = (0..n)
            .map(|i| {
                let f = i as f64;
                pt(
                    1.7e9 + f * 0.5,
                    (i % 1800) as f64 / 10.0 - 90.0,
                    (i % 3600) as f64 / 10.0 - 180.0,
                    (i % 3 != 0).then_some((i % 500) as f64 * 1.5 - 100.0),
                    (i / 1000) as u32,
                )
            })
            .collect();
        let started = std::time::Instant::now();
        let doc = write_gpx(Some("big"), &pts);
        // a loaded machine is slow, a quadratic writer would be minutes
        assert!(started.elapsed() < std::time::Duration::from_secs(30), "{:?}", started.elapsed());
        assert_eq!((doc.matches("<trkseg>").count(), doc.matches("<trkpt ").count()), (200, n));
        let log = parse_gpx(&doc).unwrap();
        assert_eq!(log.points.len(), n);
        assert_eq!(log.points.last().map(|p| (p.segment, p.time)), Some((199, 1.7e9 + 99_999.5)));
    }

    proptest::proptest! {
        /// Any valid, time-ordered track reads back within the documented precision.
        #[test]
        fn written_tracks_read_back(
            start in -2.0e9f64..4.0e9,
            steps in proptest::collection::vec(
                (0u32..100_000, 0u32..4, -90.0f64..=90.0, -180.0f64..=180.0, proptest::option::of(-500.0f64..9000.0)),
                0..40,
            ),
        ) {
            let mut t = start;
            let pts: Vec<TrackPoint> = steps
                .iter()
                .map(|&(dt, segment, lat, lon, ele)| {
                    t += f64::from(dt) / 1000.0;
                    pt(t, lat, lon, ele, segment)
                })
                .collect();
            assert_round_trip(&pts);
        }

        /// Any floats at all (hostile payloads) never panic and always give a document the reader accepts.
        #[test]
        fn arbitrary_floats_never_panic(
            raw in proptest::collection::vec(
                (proptest::num::f64::ANY, proptest::num::f64::ANY, proptest::num::f64::ANY, proptest::option::of(proptest::num::f64::ANY), proptest::num::u32::ANY),
                0..30,
            ),
            name in ".{0,40}",
        ) {
            let pts: Vec<TrackPoint> = raw.iter().map(|&(t, la, lo, e, s)| pt(t, la, lo, e, s)).collect();
            let doc = write_gpx(Some(&name), &pts);
            proptest::prop_assert!(parse_gpx(&doc).is_ok(), "{}", doc);
        }
    }
}
