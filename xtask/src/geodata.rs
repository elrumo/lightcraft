//! `cargo xtask geodata <dir>`: build `crates/geo/data/{places,land}.bin` from
//!
//! - GeoNames `cities15000.txt`, `countryInfo.txt` and `admin1CodesASCII.txt`
//!   (<https://download.geonames.org/export/dump/>, CC BY 4.0), and
//! - Natural Earth `ne_110m_land.shp` (<https://www.naturalearthdata.com/>, public domain),
//!
//! found in `<dir>`. The formats are described in `crates/geo/src/gazetteer.rs` and `land.rs`.
//! The raw dumps stay outside the repository; only the two compact files are committed.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use lightcraft_geo::normalize;

pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let dir = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .ok_or("usage: cargo xtask geodata <dir with cities15000.txt, countryInfo.txt, admin1CodesASCII.txt, ne_110m_land.shp>")?;
    let dir = Path::new(dir);
    let read = |n: &str| std::fs::read(dir.join(n)).map_err(|e| format!("{}: {e}", dir.join(n).display()));
    let text = |n: &str| read(n).map(|b| String::from_utf8_lossy(&b).into_owned());
    let places = encode_places(&text("cities15000.txt")?, &text("countryInfo.txt")?, &text("admin1CodesASCII.txt")?)?;
    let land = encode_land(&read("ne_110m_land.shp")?)?;
    let out = root.join("crates/geo/data");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    for (name, bytes) in [("places.bin", &places), ("land.bin", &land)] {
        std::fs::write(out.join(name), bytes).map_err(|e| e.to_string())?;
        println!("{}: {} bytes", out.join(name).display(), bytes.len());
    }
    Ok(())
}

fn put_varint(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    put_varint(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

fn deflate(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec(data, 10)
}

/// Other names countries go by (Spanish first: the author's users search in Spanish and English).
/// Original work: plain facts about country names.
const COUNTRY_ALIASES: &[(&str, &[&str])] = &[
    (
        "US",
        &[
            "usa",
            "u s a",
            "united states of america",
            "estados unidos",
            "ee uu",
            "eeuu",
            "etats unis",
            "vereinigte staaten",
            "stati uniti",
            "america",
            "us of a",
        ],
    ),
    (
        "GB",
        &[
            "uk",
            "u k",
            "great britain",
            "britain",
            "reino unido",
            "royaume uni",
            "grossbritannien",
            "vereinigtes konigreich",
            "regno unito",
            "inglaterra e irlanda del norte",
        ],
    ),
    ("ES", &["espana", "spanien", "espagne", "spagna", "espanha"]),
    ("DE", &["deutschland", "alemania", "allemagne", "germania", "alemanha"]),
    ("FR", &["francia", "frankreich", "franca"]),
    ("IT", &["italia", "italien", "italie"]),
    ("NL", &["holland", "paises bajos", "nederland", "niederlande", "pays bas", "olanda", "paesi bassi", "holanda"]),
    ("CH", &["suiza", "schweiz", "suisse", "svizzera", "suica"]),
    ("AT", &["osterreich", "autriche"]),
    ("BE", &["belgica", "belgien", "belgique", "belgio"]),
    ("IE", &["irlanda", "eire", "irland", "irlande"]),
    ("SE", &["suecia", "sverige", "schweden", "suede", "svezia"]),
    ("NO", &["noruega", "norge", "norwegen", "norvege", "norvegia"]),
    ("DK", &["dinamarca", "danmark", "danemark", "danimarca", "danemarca"]),
    ("FI", &["finlandia", "suomi", "finnland", "finlande"]),
    ("PL", &["polonia", "polska", "polen", "pologne"]),
    ("CZ", &["chequia", "republica checa", "czechia", "czech republic", "tschechien", "tchequie", "cesko"]),
    ("GR", &["grecia", "hellas", "griechenland", "grece", "ellada"]),
    ("TR", &["turquia", "turkiye", "turkey", "turquie", "turchia", "turkei"]),
    ("RU", &["rusia", "russia", "russland", "russie"]),
    ("UA", &["ucrania", "ukraine", "ucraina"]),
    ("JP", &["japon", "japan", "nippon", "giappone", "日本"]),
    ("CN", &["china", "中国", "中國", "chine", "cina"]),
    ("KR", &["corea del sur", "south korea", "korea", "한국", "대한민국", "coree du sud", "sudkorea"]),
    ("IN", &["india", "inde", "indien"]),
    ("MX", &["mexico", "mejico", "mexique", "messico"]),
    ("BR", &["brasil", "brazil", "bresil", "brasilien"]),
    ("MA", &["marruecos", "morocco", "maroc", "marokko", "marocco"]),
    ("EG", &["egipto", "egypt", "egypte", "agypten", "egitto"]),
    ("ZA", &["sudafrica", "south africa", "afrique du sud", "sudafrika"]),
    ("NZ", &["nueva zelanda", "new zealand", "nouvelle zelande", "neuseeland", "nuova zelanda"]),
    ("AE", &["emiratos arabes unidos", "uae", "u a e", "united arab emirates", "emirats arabes unis"]),
    ("SA", &["arabia saudita", "saudi arabia", "arabie saoudite", "saudi arabien"]),
    ("TH", &["tailandia", "thailand", "thailande"]),
    ("PH", &["filipinas", "philippines", "philippinen"]),
    ("HR", &["croacia", "croatia", "hrvatska", "kroatien", "croatie"]),
    ("HU", &["hungria", "hungary", "magyarorszag", "ungarn", "hongrie"]),
    ("RO", &["rumania", "romania", "rumanien", "roumanie"]),
    ("IS", &["islandia", "iceland", "island", "islande"]),
    ("LU", &["luxemburgo", "luxembourg", "luxemburg"]),
    ("CY", &["chipre", "cyprus", "zypern", "chypre"]),
    ("PE", &["peru"]),
    ("CO", &["colombia"]),
    ("IL", &["israel"]),
    ("VN", &["vietnam", "viet nam"]),
    ("TW", &["taiwan", "taiwán", "台灣", "台湾"]),
    ("CU", &["cuba"]),
    ("DO", &["republica dominicana", "dominican republic"]),
    ("VA", &["vaticano", "vatican", "vatican city", "ciudad del vaticano"]),
];

/// Other names cities go by that GeoNames doesn't list.
const CITY_ALIASES: &[(&str, &str, &[&str])] =
    &[("US", "New York City", &["nyc", "new york"]), ("US", "Los Angeles", &["l a"]), ("US", "San Francisco", &["sf", "san fran"])];

/// How many alternate names a city keeps: the biggest cities are searched under many spellings
/// ("Londres", "Wien"), a town of 20 000 is searched by its name.
fn alt_cap(population: u32) -> usize {
    match population {
        1_000_000.. => 60,
        200_000.. => 20,
        _ => 3,
    }
}

fn norm(s: &str) -> String {
    normalize(s)
}

/// Whether a normalised name is worth a table entry: at least two characters, not just digits,
/// and short enough to be a name.
fn usable(key: &str) -> bool {
    key.chars().count() >= 2 && key.len() <= 60 && key.chars().any(|c| !c.is_ascii_digit())
}

pub fn encode_places(cities_txt: &str, country_info: &str, admin1: &str) -> Result<Vec<u8>, String> {
    // countries: ISO code, name
    let mut countries: Vec<([u8; 2], String)> = Vec::new();
    let mut country_ix: HashMap<String, u16> = HashMap::new();
    for line in country_info.lines().filter(|l| !l.starts_with('#')) {
        let f: Vec<&str> = line.split('\t').collect();
        let (Some(code), Some(name)) = (f.first(), f.get(4)) else { continue };
        if code.len() != 2 || name.is_empty() {
            continue;
        }
        let b = code.as_bytes();
        country_ix.insert((*code).to_string(), countries.len() as u16);
        countries.push(([b[0], b[1]], (*name).to_string()));
    }
    if countries.is_empty() {
        return Err("countryInfo.txt: no countries".into());
    }
    // regions: "ES.29" → (country, name, ascii name)
    let mut regions: Vec<(u16, String, String)> = Vec::new();
    let mut region_ix: HashMap<String, u16> = HashMap::new();
    for line in admin1.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let (Some(code), Some(name)) = (f.first(), f.get(1)) else { continue };
        let Some((cc, _)) = code.split_once('.') else { continue };
        let Some(&c) = country_ix.get(cc) else { continue };
        region_ix.insert((*code).to_string(), regions.len() as u16);
        regions.push((c, (*name).to_string(), f.get(2).copied().unwrap_or(name).to_string()));
    }
    // cities, and the names each place goes by (normalised, without the one `normalize(name)` gives)
    let mut cities: Vec<(i32, i32, u32, u16, u16, String, Vec<String>)> = Vec::new();
    let mut by_country_name: HashMap<(String, String), usize> = HashMap::new();
    for line in cities_txt.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 15 {
            continue;
        }
        let (Ok(lat), Ok(lon)) = (f[4].parse::<f64>(), f[5].parse::<f64>()) else { continue };
        let Some(&country) = country_ix.get(f[8]) else { continue };
        let population = f[14].parse::<u32>().unwrap_or(0);
        let region = region_ix.get(&format!("{}.{}", f[8], f[10])).copied().unwrap_or(u16::MAX);
        let mut seen: BTreeSet<String> = BTreeSet::from([norm(f[1])]);
        let mut others = Vec::new();
        for alt in std::iter::once(f[2]).chain(f[3].split(',')) {
            if others.len() > alt_cap(population) {
                break;
            }
            let k = norm(alt);
            if usable(&k) && seen.insert(k.clone()) {
                others.push(k);
            }
        }
        let index = cities.len();
        by_country_name
            .entry((f[8].to_string(), f[1].to_string()))
            .and_modify(|i| {
                if cities.get(*i).is_some_and(|c| c.2 < population) {
                    *i = index;
                }
            })
            .or_insert(index);
        cities.push(((lat * 1e5).round() as i32, (lon * 1e5).round() as i32, population, country, region, f[1].to_string(), others));
    }
    for (cc, name, aliases) in CITY_ALIASES {
        if let Some(&i) = by_country_name.get(&((*cc).to_string(), (*name).to_string())) {
            let own = norm(name);
            if let Some(c) = cities.get_mut(i) {
                c.6.extend(aliases.iter().map(|a| norm(a)).filter(|a| *a != own));
            }
        }
    }
    let region_names: Vec<Vec<String>> =
        regions.iter().map(|(_, name, ascii)| Some(norm(ascii)).filter(|a| usable(a) && *a != norm(name)).into_iter().collect()).collect();
    let country_names: Vec<Vec<String>> = countries
        .iter()
        .map(|(code, name)| {
            let cc = String::from_utf8_lossy(code).into_owned();
            let own = norm(name);
            let mut v: Vec<String> = COUNTRY_ALIASES
                .iter()
                .filter(|(c, _)| *c == cc)
                .flat_map(|(_, a)| a.iter().map(|x| norm(x)))
                .filter(|a| usable(a) && *a != own)
                .collect();
            v.sort();
            v.dedup();
            v
        })
        .collect();

    let put_names = |out: &mut Vec<u8>, names: &[String]| {
        put_varint(out, names.len() as u32);
        for n in names {
            put_str(out, n);
        }
    };
    let mut out = Vec::new();
    out.extend_from_slice(b"LCG2");
    out.extend_from_slice(&(countries.len() as u32).to_le_bytes());
    for ((code, name), names) in countries.iter().zip(&country_names) {
        out.extend_from_slice(code);
        put_str(&mut out, name);
        put_names(&mut out, names);
    }
    out.extend_from_slice(&(regions.len() as u32).to_le_bytes());
    for ((c, name, _), names) in regions.iter().zip(&region_names) {
        out.extend_from_slice(&c.to_le_bytes());
        put_str(&mut out, name);
        put_names(&mut out, names);
    }
    out.extend_from_slice(&(cities.len() as u32).to_le_bytes());
    for (lat, lon, pop, country, region, name, names) in &cities {
        out.extend_from_slice(&lat.to_le_bytes());
        out.extend_from_slice(&lon.to_le_bytes());
        put_varint(&mut out, *pop);
        out.extend_from_slice(&country.to_le_bytes());
        out.extend_from_slice(&region.to_le_bytes());
        put_str(&mut out, name);
        put_names(&mut out, names);
    }
    Ok(deflate(&out))
}

// ---- Natural Earth coastlines -------------------------------------------------------------

type Ring = Vec<(f64, f64)>;

/// The polygon rings of each record of an ESRI shapefile (type 5 = Polygon).
fn read_shp(b: &[u8]) -> Result<Vec<Vec<Ring>>, String> {
    let be32 = |o: usize| b.get(o..o + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]) as usize);
    let le32 = |o: usize| b.get(o..o + 4).map(|s| i32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let f64le = |o: usize| b.get(o..o + 8).map(|s| f64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]));
    if be32(0) != Some(9994) {
        return Err("not a shapefile".into());
    }
    let mut records = Vec::new();
    let mut pos = 100;
    while pos + 8 <= b.len() {
        let len = be32(pos + 4).ok_or("truncated")? * 2;
        let c = pos + 8;
        pos = c + len;
        if le32(c) != Some(5) {
            continue;
        }
        let parts = le32(c + 36).ok_or("truncated")? as usize;
        let points = le32(c + 40).ok_or("truncated")? as usize;
        let starts: Vec<usize> = (0..parts).map(|i| le32(c + 44 + 4 * i).map(|v| v as usize).ok_or("truncated")).collect::<Result<_, _>>()?;
        let pts_at = c + 44 + 4 * parts;
        let pts: Vec<(f64, f64)> = (0..points)
            .map(|i| Ok((f64le(pts_at + 16 * i).ok_or("truncated")?, f64le(pts_at + 16 * i + 8).ok_or("truncated")?)))
            .collect::<Result<_, String>>()?;
        let mut rings = Vec::new();
        for (i, s) in starts.iter().enumerate() {
            let e = starts.get(i + 1).copied().unwrap_or(points);
            let mut ring: Ring = pts.get(*s..e).ok_or("bad part")?.to_vec();
            if ring.len() > 1 && ring.first() == ring.last() {
                ring.pop();
            }
            if ring.len() >= 3 {
                rings.push(ring);
            }
        }
        records.push(rings);
    }
    Ok(records)
}

/// Shoelace area: negative for a clockwise ring (a shapefile's outer boundary), positive for a hole.
fn area(r: &Ring) -> f64 {
    r.iter().zip(r.iter().cycle().skip(1)).map(|(a, b)| a.0 * b.1 - b.0 * a.1).sum::<f64>() / 2.0
}

fn inside(poly: &Ring, p: (f64, f64)) -> bool {
    let mut c = false;
    for (a, b) in poly.iter().zip(poly.iter().cycle().skip(1)) {
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            c = !c;
        }
    }
    c
}

pub fn encode_land(shp: &[u8]) -> Result<Vec<u8>, String> {
    let mut vertices: Vec<(i16, i16)> = Vec::new();
    let mut indices: Vec<u16> = Vec::new();
    let q = |v: f64| (v * 100.0).round().clamp(-32768.0, 32767.0) as i16;
    for rings in read_shp(shp)? {
        let (outer, holes): (Vec<&Ring>, Vec<&Ring>) = rings.iter().partition(|r| area(r) < 0.0);
        for o in outer {
            let mine: Vec<&Ring> = holes.iter().copied().filter(|h| h.first().is_some_and(|p| inside(o, *p))).collect();
            let mut flat: Vec<f64> = o.iter().flat_map(|p| [p.0, p.1]).collect();
            let mut hole_at = Vec::new();
            for h in &mine {
                hole_at.push(flat.len() / 2);
                flat.extend(h.iter().flat_map(|p| [p.0, p.1]));
            }
            let tris = earcutr::earcut(&flat, &hole_at, 2).map_err(|e| format!("triangulation: {e:?}"))?;
            let base = vertices.len();
            vertices.extend(flat.as_chunks::<2>().0.iter().map(|c| (q(c[0]), q(c[1]))));
            for t in tris.as_chunks::<3>().0 {
                let v = |i: usize| vertices.get(base + i).copied().unwrap_or((0, 0));
                let (a, b, c) = (v(t[0]), v(t[1]), v(t[2]));
                // rounding to 0.01° can collapse a sliver into a point or a line: drop it
                let twice_area = (i32::from(b.0) - i32::from(a.0)) * (i32::from(c.1) - i32::from(a.1))
                    - (i32::from(c.0) - i32::from(a.0)) * (i32::from(b.1) - i32::from(a.1));
                if twice_area == 0 {
                    continue;
                }
                for &i in t.iter() {
                    indices.push(u16::try_from(base + i).map_err(|_| "too many vertices for u16 indices".to_string())?);
                }
            }
        }
    }
    if vertices.is_empty() || indices.is_empty() {
        return Err("no land".into());
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"LCL1");
    out.extend_from_slice(&(vertices.len() as u32).to_le_bytes());
    for (lon, lat) in &vertices {
        out.extend_from_slice(&lon.to_le_bytes());
        out.extend_from_slice(&lat.to_le_bytes());
    }
    out.extend_from_slice(&(indices.len() as u32).to_le_bytes());
    for i in &indices {
        out.extend_from_slice(&i.to_le_bytes());
    }
    Ok(deflate(&out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_geo::land::Land;
    use lightcraft_geo::{Gazetteer, Kind};

    const COUNTRIES: &str = "# comment\nES\tESP\t724\tSP\tSpain\tMadrid\nFR\tFRA\t250\tFR\tFrance\tParis\n";
    const ADMIN1: &str = "ES.29\tMadrid\tMadrid\t3117732\nFR.11\tÎle-de-France\tIle-de-France\t3012874\n";
    fn city(id: u32, name: &str, ascii: &str, alts: &str, lat: f64, lon: f64, cc: &str, a1: &str, pop: u32) -> String {
        format!("{id}\t{name}\t{ascii}\t{alts}\t{lat}\t{lon}\tP\tPPL\t{cc}\t\t{a1}\t\t\t\t{pop}\t\t\tEurope/Madrid\t2020-01-01")
    }

    #[test]
    fn places_round_trip_through_the_reader() {
        let cities = [
            city(1, "Madrid", "Madrid", "Madrid,Mad,Londres,マドリード", 40.4165, -3.70256, "ES", "29", 3_255_944),
            city(2, "Alcalá de Henares", "Alcala de Henares", "", 40.4818, -3.3643, "ES", "29", 195_000),
            city(3, "Paris", "Paris", "Parigi", 48.85341, 2.3488, "FR", "11", 2_138_551),
        ]
        .join("\n");
        let g = Gazetteer::from_bytes(&encode_places(&cities, COUNTRIES, ADMIN1).unwrap()).unwrap();
        assert_eq!(g.cities().len(), 3);
        let ids = g.lookup_text("MADRID");
        assert!(ids.iter().any(|i| i.kind == Kind::City) && ids.iter().any(|i| i.kind == Kind::Region), "{ids:?}");
        assert_eq!(g.lookup_text("alcala de henares").len(), 1);
        assert_eq!(g.lookup_text("マドリード").len(), 1);
        assert_eq!(g.lookup_text("parigi").len(), 1);
        assert_eq!(g.lookup_text("españa").len(), 1, "alias");
        assert_eq!(g.lookup_text("ile de france").len(), 1);
        assert!(g.lookup_text("nowhere").is_empty());
        let n = g.name_of(40.42, -3.7).unwrap();
        assert_eq!((n.city, n.region, n.country), (Some("Madrid"), Some("Madrid"), Some("Spain")));
        assert_eq!(n.display(), "Madrid, Spain");
    }

    #[test]
    fn broken_lines_are_skipped() {
        let cities = [
            "short\tline",
            "x\tName\tName\t\tnot-a-number\t0\tP\tPPL\tES\t\t29\t\t\t\t1\t\t\tz\td",
            &city(1, "Madrid", "Madrid", "", 40.4, -3.7, "ES", "29", 100),
            &city(2, "Atlantis", "Atlantis", "", 0.0, 0.0, "XX", "", 100),
        ]
        .join("\n");
        let g = Gazetteer::from_bytes(&encode_places(&cities, COUNTRIES, ADMIN1).unwrap()).unwrap();
        assert_eq!(g.cities().len(), 1);
        assert!(encode_places("", "", "").is_err());
    }

    /// A one-record polygon shapefile: a 10° square with a 2° hole.
    fn square_shp() -> Vec<u8> {
        let outer: [(f64, f64); 5] = [(0.0, 0.0), (0.0, 10.0), (10.0, 10.0), (10.0, 0.0), (0.0, 0.0)]; // clockwise
        let hole: [(f64, f64); 5] = [(4.0, 4.0), (6.0, 4.0), (6.0, 6.0), (4.0, 6.0), (4.0, 4.0)]; // counter-clockwise
        let mut c = Vec::new();
        c.extend_from_slice(&5i32.to_le_bytes());
        for v in [0.0f64, 0.0, 10.0, 10.0] {
            c.extend_from_slice(&v.to_le_bytes());
        }
        c.extend_from_slice(&2i32.to_le_bytes());
        c.extend_from_slice(&10i32.to_le_bytes());
        c.extend_from_slice(&0i32.to_le_bytes());
        c.extend_from_slice(&5i32.to_le_bytes());
        for p in outer.iter().chain(hole.iter()) {
            c.extend_from_slice(&p.0.to_le_bytes());
            c.extend_from_slice(&p.1.to_le_bytes());
        }
        let mut f = vec![0u8; 100];
        f[..4].copy_from_slice(&9994u32.to_be_bytes());
        f.extend_from_slice(&1u32.to_be_bytes());
        f.extend_from_slice(&((c.len() / 2) as u32).to_be_bytes());
        f.extend_from_slice(&c);
        f
    }

    #[test]
    fn land_is_triangulated_around_holes() {
        let land = Land::from_bytes(&encode_land(&square_shp()).unwrap()).unwrap();
        // area of the triangles = 10×10 − 2×2
        let a: f64 = land.triangles().map(|[p, q, r]| f64::from(((q.1 - p.1) * (r.0 - p.0) - (r.1 - p.1) * (q.0 - p.0)).abs()) / 2.0).sum();
        assert!((a - 96.0).abs() < 0.01, "{a}");
        assert!(encode_land(&[1, 2, 3]).is_err());
    }
}
