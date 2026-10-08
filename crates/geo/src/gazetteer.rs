//! The offline gazetteer: ~34 000 cities (GeoNames, population ≥ 15 000) with their regions and
//! countries, a table of every name they go by (accent- and case-insensitive), and a grid for
//! nearest-city lookups.
//!
//! Two directions, both without a network:
//! - **forward** — [`Gazetteer::lookup`]: "madrid" → the cities, regions and countries so named;
//! - **reverse** — [`Gazetteer::locate`] / [`Gazetteer::name_of`]: a GPS position → the nearest city.
//!
//! The data is `data/places.bin`, written by `cargo xtask geodata` and embedded in the binary.
//! File layout (after inflating; all integers little-endian, `str` = LEB128 length + UTF-8, `names`
//! = a LEB128 count followed by that many `str`s, the already [`normalize`]d other names of the
//! place, without the one `normalize(name)` gives):
//!
//! ```text
//! "LCG2"
//! u32 n; n × { [u8;2] ISO code, str name, names }                countries
//! u32 n; n × { u16 country, str name, names }                    regions (GeoNames admin 1)
//! u32 n; n × { i32 lat·1e5, i32 lon·1e5, varint population,
//!              u16 country, u16 region (0xFFFF none), str name, names } cities
//! ```
//!
//! The table of names is sorted when the file is read: storing it sorted would cost a position
//! per name, about a third of the file.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::codec::{DataError, Reader, inflate};
use crate::geodesy::{distance_km, valid};
use crate::text::normalize;

const MAGIC: &[u8; 4] = b"LCG2";
/// "No region" in a city record.
pub const NO_REGION: u16 = u16::MAX;
/// A photo further than this from every city has no place (open sea, polar regions, deep desert).
pub const LOCATE_KM: f64 = 150.0;
/// Within this distance of a city a photo is "in" that city for display purposes.
pub const IN_CITY_KM: f64 = 50.0;
/// Longest name (in words) worth looking up: "Saint Petersburg", "Rio de Janeiro", "Santa Cruz de Tenerife".
pub const MAX_NAME_WORDS: usize = 5;

static EMBEDDED: &[u8] = include_bytes!("../data/places.bin");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    City,
    Region,
    Country,
}

/// A city, region or country of the gazetteer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlaceId {
    pub kind: Kind,
    pub index: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct City {
    pub name: Box<str>,
    pub lat: f64,
    pub lon: f64,
    pub population: u32,
    /// Index into the countries.
    pub country: u16,
    /// Index into the regions, or [`NO_REGION`].
    pub region: u16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Country {
    /// ISO 3166-1 alpha-2, upper case.
    pub code: [u8; 2],
    pub name: Box<str>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub country: u16,
    pub name: Box<str>,
}

#[derive(Clone, Copy, Debug)]
struct Key {
    /// The first eight bytes of the name, big-endian: most comparisons end here.
    prefix: u64,
    off: u32,
    len: u16,
    kind: Kind,
    index: u32,
}

/// The nearest city to a position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Located {
    /// Index of the city.
    pub city: u32,
    pub distance_km: f64,
}

/// Where a position is, in words.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaceName<'a> {
    /// The city, when the position is within [`IN_CITY_KM`] of one.
    pub city: Option<&'a str>,
    pub region: Option<&'a str>,
    pub country: Option<&'a str>,
    pub country_code: Option<[u8; 2]>,
    /// Distance to the nearest city, kilometres.
    pub distance_km: f64,
}

impl PlaceName<'_> {
    /// "Madrid, Madrid, Spain" — city, region (unless it repeats the city), country.
    pub fn display(&self) -> String {
        let mut parts: Vec<&str> = Vec::with_capacity(3);
        if let Some(c) = self.city {
            parts.push(c);
        }
        if let Some(r) = self.region.filter(|r| !r.is_empty() && self.city != Some(*r)) {
            parts.push(r);
        }
        if let Some(c) = self.country {
            parts.push(c);
        }
        parts.join(", ")
    }
}

/// The first eight bytes of `k` as a big-endian number (zero-padded): byte order and number order agree.
fn prefix_of(k: &[u8]) -> u64 {
    let mut b = [0u8; 8];
    for (d, s) in b.iter_mut().zip(k) {
        *d = *s;
    }
    u64::from_be_bytes(b)
}

const GRID_W: usize = 360;
const GRID_H: usize = 180;

pub struct Gazetteer {
    countries: Vec<Country>,
    regions: Vec<Region>,
    cities: Vec<City>,
    /// The normalised names, back to back; the keys point into it.
    key_text: Vec<u8>,
    /// Every name of every place, sorted by name.
    keys: Vec<Key>,
    /// Prefix offsets into `grid_cities`, one cell per degree (`GRID_W * GRID_H + 1` entries).
    grid_start: Vec<u32>,
    grid_cities: Vec<u32>,
    /// [`Gazetteer::locate_cell`]'s results by 0.01° cell.
    cells: Mutex<HashMap<(i32, i32), Option<Located>>>,
}

impl Gazetteer {
    /// A gazetteer that knows nothing (what the app uses if its data were ever unreadable).
    pub fn empty() -> Gazetteer {
        Gazetteer {
            countries: vec![],
            regions: vec![],
            cities: vec![],
            key_text: vec![],
            keys: vec![],
            grid_start: vec![0; GRID_W * GRID_H + 1],
            grid_cities: vec![],
            cells: Mutex::default(),
        }
    }

    /// The gazetteer built into the app, parsed on first use (about 100 ms natively).
    pub fn global() -> &'static Gazetteer {
        static G: OnceLock<Gazetteer> = OnceLock::new();
        G.get_or_init(|| Gazetteer::from_bytes(EMBEDDED).unwrap_or_else(|_| Gazetteer::empty()))
    }

    /// Parse a deflated `places.bin`.
    pub fn from_bytes(deflated: &[u8]) -> Result<Gazetteer, DataError> {
        let data = inflate(deflated)?;
        let mut r = Reader::new(&data);
        if r.take(4)? != MAGIC {
            return Err(DataError("not a place file"));
        }
        let mut key_text: Vec<u8> = Vec::with_capacity(data.len());
        let mut keys: Vec<Key> = Vec::new();
        // The names of one place: `normalize(name)` and the stored others.
        let mut names = |r: &mut Reader<'_>, name: &str, kind: Kind, index: u32| -> Result<(), DataError> {
            let mut push = |k: &str| {
                if let (Ok(off), Ok(len)) = (u32::try_from(key_text.len()), u16::try_from(k.len())) {
                    key_text.extend_from_slice(k.as_bytes());
                    keys.push(Key { prefix: prefix_of(k.as_bytes()), off, len, kind, index });
                }
            };
            push(&normalize(name));
            // each name takes at least its length byte
            let n = r.varint()? as usize;
            if n > data.len() {
                return Err(DataError("implausible count"));
            }
            for _ in 0..n {
                push(r.str_span()?);
            }
            Ok(())
        };
        let n = r.count(4)?;
        let mut countries = Vec::with_capacity(n);
        for i in 0..n {
            let c = r.take(2)?;
            let code = [c.first().copied().unwrap_or(b'?'), c.get(1).copied().unwrap_or(b'?')];
            let name = r.str_span()?;
            names(&mut r, name, Kind::Country, i as u32)?;
            countries.push(Country { code, name: name.into() });
        }
        let n = r.count(4)?;
        let mut regions = Vec::with_capacity(n);
        for i in 0..n {
            let country = r.u16()?;
            let name = r.str_span()?;
            names(&mut r, name, Kind::Region, i as u32)?;
            regions.push(Region { country, name: name.into() });
        }
        let n = r.count(14)?;
        let mut cities = Vec::with_capacity(n);
        for i in 0..n {
            let lat = f64::from(r.i32()?) / 1e5;
            let lon = f64::from(r.i32()?) / 1e5;
            let population = r.varint()?;
            let country = r.u16()?;
            let region = r.u16()?;
            if usize::from(country) >= countries.len() || (region != NO_REGION && usize::from(region) >= regions.len()) {
                return Err(DataError("dangling reference"));
            }
            let name = r.str_span()?;
            names(&mut r, name, Kind::City, i as u32)?;
            cities.push(City { name: name.into(), lat, lon, population, country, region });
        }
        let bytes_of = |k: &Key| key_text.get(k.off as usize..k.off as usize + usize::from(k.len)).unwrap_or(&[]);
        keys.sort_unstable_by(|a, b| {
            a.prefix.cmp(&b.prefix).then_with(|| bytes_of(a).cmp(bytes_of(b))).then(a.kind.cmp(&b.kind)).then(a.index.cmp(&b.index))
        });
        keys.dedup_by(|b, a| a.kind == b.kind && a.index == b.index && a.prefix == b.prefix && bytes_of(a) == bytes_of(b));
        keys.retain(|k| k.len > 0);
        keys.shrink_to_fit();
        let mut g = Gazetteer { countries, regions, cities, key_text, keys, grid_start: vec![], grid_cities: vec![], cells: Mutex::default() };
        g.build_grid();
        Ok(g)
    }

    fn cell(lat: f64, lon: f64) -> usize {
        let row = ((lat + 90.0).floor() as i64).clamp(0, GRID_H as i64 - 1) as usize;
        let col = ((lon + 180.0).floor() as i64).clamp(0, GRID_W as i64 - 1) as usize;
        row * GRID_W + col
    }

    fn build_grid(&mut self) {
        let mut start = vec![0u32; GRID_W * GRID_H + 1];
        for c in &self.cities {
            if let Some(s) = start.get_mut(Self::cell(c.lat, c.lon) + 1) {
                *s += 1;
            }
        }
        for i in 1..start.len() {
            start[i] += start[i - 1];
        }
        let mut fill = start.clone();
        let mut order = vec![0u32; self.cities.len()];
        for (i, c) in self.cities.iter().enumerate() {
            if let Some(f) = fill.get_mut(Self::cell(c.lat, c.lon))
                && let Some(o) = order.get_mut(*f as usize)
            {
                *o = i as u32;
                *f += 1;
            }
        }
        self.grid_start = start;
        self.grid_cities = order;
    }

    pub fn is_empty(&self) -> bool {
        self.cities.is_empty()
    }

    pub fn cities(&self) -> &[City] {
        &self.cities
    }

    pub fn city(&self, index: u32) -> Option<&City> {
        self.cities.get(index as usize)
    }

    pub fn country(&self, index: u16) -> Option<&Country> {
        self.countries.get(usize::from(index))
    }

    pub fn region(&self, index: u16) -> Option<&Region> {
        self.regions.get(usize::from(index))
    }

    fn key_bytes(&self, k: &Key) -> &[u8] {
        let s = k.off as usize;
        self.key_text.get(s..s + usize::from(k.len)).unwrap_or(&[])
    }

    /// Every city, region and country called `key` (already [`normalize`]d).
    pub fn lookup(&self, key: &str) -> Vec<PlaceId> {
        let k = key.as_bytes();
        if k.is_empty() {
            return vec![];
        }
        let p = prefix_of(k);
        let order = |e: &Key| e.prefix.cmp(&p).then_with(|| self.key_bytes(e).cmp(k));
        let first = self.keys.partition_point(|e| order(e).is_lt());
        self.keys.iter().skip(first).take_while(|e| order(e).is_eq()).map(|e| PlaceId { kind: e.kind, index: e.index }).collect()
    }

    /// [`lookup`](Self::lookup) for text as a person typed it ("Alcalá de Henares", "NEW-YORK").
    pub fn lookup_text(&self, text: &str) -> Vec<PlaceId> {
        self.lookup(&normalize(text))
    }

    /// "Madrid, Spain", "Community of Madrid, Spain", "Spain".
    pub fn label(&self, id: PlaceId) -> Option<String> {
        match id.kind {
            Kind::City => {
                let c = self.city(id.index)?;
                let region = self.region(c.region).map(|r| &*r.name).filter(|r| *r != &*c.name);
                let country = self.country(c.country).map(|c| &*c.name);
                Some([Some(&*c.name), region, country].into_iter().flatten().collect::<Vec<_>>().join(", "))
            }
            Kind::Region => {
                let r = self.region(u16::try_from(id.index).ok()?)?;
                let country = self.country(r.country).map(|c| &*c.name);
                Some([Some(&*r.name), country].into_iter().flatten().collect::<Vec<_>>().join(", "))
            }
            Kind::Country => self.country(u16::try_from(id.index).ok()?).map(|c| c.name.to_string()),
        }
    }

    /// The nearest city within `max_km` of a position, and how far it is.
    pub fn nearest(&self, lat: f64, lon: f64, max_km: f64) -> Option<Located> {
        if !valid(lat, lon) || self.cities.is_empty() || max_km.is_nan() || max_km <= 0.0 {
            return None;
        }
        let mut reach_deg = 1.0_f64;
        loop {
            // Scan every cell the disk of `reach_deg` degrees of latitude can touch; the disk is
            // wider in degrees of longitude towards the poles.
            let reach_km = reach_deg * 110.0;
            let best = self.scan(lat, lon, reach_deg);
            if let Some(b) = best
                && b.distance_km <= reach_km.min(max_km)
            {
                return Some(b);
            }
            if reach_km >= max_km || reach_deg >= 90.0 {
                return best.filter(|b| b.distance_km <= max_km);
            }
            reach_deg *= 2.0;
        }
    }

    fn scan(&self, lat: f64, lon: f64, reach_deg: f64) -> Option<Located> {
        let cos_lat = lat.to_radians().cos();
        let lon_span = (reach_deg / cos_lat.max(0.01) * 1.1).min(180.0);
        let rows = (((lat + 90.0 - reach_deg).floor() as i64).max(0))..=(((lat + 90.0 + reach_deg).floor() as i64).min(GRID_H as i64 - 1));
        let (c0, c1) = if lon_span >= 180.0 {
            (0, GRID_W as i64 - 1)
        } else {
            ((lon + 180.0 - lon_span).floor() as i64, (lon + 180.0 + lon_span).floor() as i64)
        };
        // Candidates are compared by a flat-earth distance in degrees²; only the winner gets the
        // exact (and ten times dearer) great-circle distance.
        let mut best: Option<(f64, u32)> = None;
        for row in rows {
            for c in c0..=c1 {
                let cell = row as usize * GRID_W + c.rem_euclid(GRID_W as i64) as usize;
                let (Some(&a), Some(&b)) = (self.grid_start.get(cell), self.grid_start.get(cell + 1)) else { continue };
                for &ci in self.grid_cities.get(a as usize..b as usize).unwrap_or(&[]) {
                    let Some(city) = self.city(ci) else { continue };
                    let dlat = city.lat - lat;
                    let mut dlon = (city.lon - lon).abs();
                    if dlon > 180.0 {
                        dlon = 360.0 - dlon;
                    }
                    let d2 = dlat * dlat + (dlon * cos_lat) * (dlon * cos_lat);
                    if best.is_none_or(|(b, _)| d2 < b) {
                        best = Some((d2, ci));
                    }
                }
            }
        }
        let (_, ci) = best?;
        let city = self.city(ci)?;
        Some(Located { city: ci, distance_km: distance_km(lat, lon, city.lat, city.lon) })
    }

    /// The nearest city within [`LOCATE_KM`] (the one a photo's region and country come from).
    pub fn locate(&self, lat: f64, lon: f64) -> Option<Located> {
        self.nearest(lat, lon, LOCATE_KM)
    }

    /// [`locate`](Self::locate) for the 0.01° cell (about a kilometre) the position is in, answered
    /// for the cell's centre and remembered: a library has thousands of photos per place, and the
    /// search asks about every one of them.
    pub fn locate_cell(&self, lat: f64, lon: f64) -> Option<Located> {
        if !valid(lat, lon) {
            return None;
        }
        let cell = ((lat * 100.0).floor() as i32, (lon * 100.0).floor() as i32);
        if let Some(hit) = self.cells.lock().unwrap_or_else(PoisonError::into_inner).get(&cell) {
            return *hit;
        }
        let found = self.locate((f64::from(cell.0) + 0.5) / 100.0, (f64::from(cell.1) + 0.5) / 100.0);
        let mut cells = self.cells.lock().unwrap_or_else(PoisonError::into_inner);
        if cells.len() >= 1 << 18 {
            cells.clear();
        }
        cells.insert(cell, found);
        found
    }

    /// Where a GPS position is: city (within [`IN_CITY_KM`]), region and country (within [`LOCATE_KM`]).
    pub fn name_of(&self, lat: f64, lon: f64) -> Option<PlaceName<'_>> {
        let l = self.locate(lat, lon)?;
        let c = self.city(l.city)?;
        let country = self.country(c.country);
        Some(PlaceName {
            city: (l.distance_km <= IN_CITY_KM).then_some(&*c.name),
            region: self.region(c.region).map(|r| &*r.name),
            country: country.map(|c| &*c.name),
            country_code: country.map(|c| c.code),
            distance_km: l.distance_km,
        })
    }
}

/// How far from a city's centre its metropolitan area is taken to reach, kilometres: a search for
/// "madrid" should find the photos taken in Alcobendas and Getafe too.
pub fn city_radius_km(population: u32) -> f64 {
    match population {
        5_000_000.. => 40.0,
        1_000_000.. => 30.0,
        300_000.. => 20.0,
        100_000.. => 14.0,
        _ => 9.0,
    }
}

/// A set of places compiled for testing many positions quickly.
///
/// A city matches the positions within [`city_radius_km`] of its centre; a region or a country
/// matches the positions whose nearest city ([`Gazetteer::locate`]) lies in it.
#[derive(Clone, Debug, Default)]
pub struct PlaceFilter {
    ids: Vec<PlaceId>,
    circles: Vec<Circle>,
    regions: Vec<u16>,
    countries: Vec<u16>,
}

#[derive(Clone, Copy, Debug)]
struct Circle {
    lat: f64,
    lon: f64,
    radius_km: f64,
    /// Bounding box in degrees, a cheap test before the haversine.
    dlat: f64,
    dlon: f64,
}

impl PlaceFilter {
    pub fn new(g: &Gazetteer, places: &[PlaceId]) -> PlaceFilter {
        let mut f = PlaceFilter { ids: places.to_vec(), ..PlaceFilter::default() };
        for p in places {
            match p.kind {
                Kind::City => {
                    if let Some(c) = g.city(p.index) {
                        let radius_km = city_radius_km(c.population);
                        let dlat = radius_km / 110.0;
                        let dlon = (radius_km / (110.0 * c.lat.to_radians().cos().max(0.01))).min(180.0);
                        f.circles.push(Circle { lat: c.lat, lon: c.lon, radius_km, dlat, dlon });
                    }
                }
                Kind::Region => f.regions.extend(u16::try_from(p.index)),
                Kind::Country => f.countries.extend(u16::try_from(p.index)),
            }
        }
        f
    }

    pub fn is_empty(&self) -> bool {
        self.circles.is_empty() && self.regions.is_empty() && self.countries.is_empty()
    }

    /// The places this filter was made from.
    pub fn places(&self) -> &[PlaceId] {
        &self.ids
    }

    /// Whether `name` (a city, region or country written in a photo's IPTC fields: "Madrid",
    /// "España") names one of the places.
    pub fn names(&self, g: &Gazetteer, name: &str) -> bool {
        !name.trim().is_empty() && g.lookup_text(name).iter().any(|f| self.ids.contains(f))
    }

    /// Whether a position is in any of the places.
    pub fn contains(&self, g: &Gazetteer, lat: f64, lon: f64) -> bool {
        if !valid(lat, lon) {
            return false;
        }
        for c in &self.circles {
            let dlon = (lon - c.lon).abs();
            if (lat - c.lat).abs() <= c.dlat && (dlon.min(360.0 - dlon)) <= c.dlon && distance_km(lat, lon, c.lat, c.lon) <= c.radius_km {
                return true;
            }
        }
        if self.regions.is_empty() && self.countries.is_empty() {
            return false;
        }
        let Some(city) = g.locate_cell(lat, lon).and_then(|l| g.city(l.city)) else { return false };
        self.regions.contains(&city.region) || self.countries.contains(&city.country)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g() -> &'static Gazetteer {
        Gazetteer::global()
    }

    #[test]
    fn the_embedded_data_loads() {
        assert!(g().cities().len() > 30_000, "{}", g().cities().len());
    }

    #[test]
    fn madrid_means_the_capital_of_spain() {
        let ids = g().lookup_text("Madrid");
        let labels: Vec<String> = ids.iter().filter_map(|i| g().label(*i)).collect();
        assert!(labels.iter().any(|l| l.ends_with("Spain")), "{labels:?}");
        assert!(ids.iter().any(|i| i.kind == Kind::Region), "the community of Madrid is a region: {labels:?}");
    }

    #[test]
    fn names_are_accent_and_case_insensitive() {
        assert_eq!(g().lookup_text("ALCALÁ DE HENARES"), g().lookup_text("alcala de henares"));
        assert!(!g().lookup_text("alcala de henares").is_empty());
        assert!(!g().lookup_text("Köln").is_empty() && !g().lookup_text("koln").is_empty());
    }

    #[test]
    fn well_known_alternate_names() {
        for n in ["Londres", "Wien", "Roma", "Praha", "東京", "Москва", "New York", "Rio de Janeiro", "San Francisco"] {
            assert!(!g().lookup_text(n).is_empty(), "{n}");
        }
        assert!(!g().lookup_text("España").is_empty() && !g().lookup_text("Spain").is_empty());
        assert!(!g().lookup_text("USA").is_empty() && !g().lookup_text("Estados Unidos").is_empty());
        assert!(g().lookup_text("zzzz nowhere").is_empty());
        assert!(g().lookup("").is_empty());
    }

    #[test]
    fn locates_the_nearest_city() {
        // Puerta del Sol
        let n = g().name_of(40.4169, -3.7035).unwrap();
        assert_eq!(n.city, Some("Madrid"));
        assert_eq!(n.country, Some("Spain"));
        assert_eq!(n.country_code, Some(*b"ES"));
        assert!(n.display().ends_with("Spain"), "{}", n.display());
        // Eiffel Tower
        assert_eq!(g().name_of(48.8584, 2.2945).unwrap().country, Some("France"));
        // Sydney Opera House, south of the equator and east of Greenwich
        assert_eq!(g().name_of(-33.8568, 151.2153).unwrap().country, Some("Australia"));
        // Hawaii, across the antimeridian from Fiji
        assert_eq!(g().name_of(21.3, -157.85).unwrap().country, Some("United States"));
    }

    #[test]
    fn the_middle_of_the_ocean_has_no_place() {
        assert!(g().name_of(0.0, -140.0).is_none());
        assert!(g().name_of(-60.0, 0.0).is_none());
    }

    #[test]
    fn hostile_positions_are_just_none() {
        for (la, lo) in [(f64::NAN, 0.0), (0.0, f64::INFINITY), (95.0, 0.0), (0.0, 400.0), (-90.0, -180.0), (90.0, 180.0)] {
            let _ = g().nearest(la, lo, 150.0);
        }
        assert!(g().nearest(40.0, -3.0, 0.0).is_none());
        assert!(g().nearest(40.0, -3.0, f64::NAN).is_none());
        assert!(Gazetteer::empty().name_of(40.0, -3.0).is_none());
        assert!(Gazetteer::from_bytes(&[1, 2, 3]).is_err());
        assert!(Gazetteer::from_bytes(&[]).is_err());
    }

    #[test]
    fn near_the_poles_and_the_antimeridian() {
        // Longyearbyen (Svalbard) and Anadyr / Provideniya (just west of the antimeridian)
        assert!(g().locate(78.22, 15.65).is_some());
        assert!(g().locate(64.73, 177.5).is_some());
        // the same place, seen from either side of 180°
        let a = g().locate(64.73, 179.99).map(|l| l.city);
        let b = g().locate(64.73, -179.99).map(|l| l.city);
        assert!(a.is_some() && b.is_some());
    }

    #[test]
    fn a_city_filter_covers_its_suburbs() {
        let madrid = g().lookup_text("madrid");
        let f = PlaceFilter::new(g(), &madrid);
        assert!(f.contains(g(), 40.4169, -3.7035), "centre");
        assert!(f.contains(g(), 40.5400, -3.6400), "Alcobendas");
        assert!(f.contains(g(), 40.3000, -3.7300), "Getafe");
        assert!(!f.contains(g(), 41.3851, 2.1734), "Barcelona");
        assert!(!f.contains(g(), 51.5, -0.12), "London");
        assert!(!f.contains(g(), f64::NAN, 0.0));
    }

    #[test]
    fn a_country_filter_follows_the_nearest_city() {
        let spain = PlaceFilter::new(g(), &g().lookup_text("españa"));
        assert!(spain.contains(g(), 41.3851, 2.1734), "Barcelona");
        assert!(spain.contains(g(), 37.3891, -5.9845), "Seville");
        assert!(!spain.contains(g(), 48.8566, 2.3522), "Paris");
        assert!(!PlaceFilter::new(g(), &[]).contains(g(), 40.4, -3.7));
    }

    #[test]
    fn iptc_names_match_by_meaning() {
        let spain = PlaceFilter::new(g(), &g().lookup_text("spain"));
        assert!(spain.names(g(), "España") && spain.names(g(), "Spain") && !spain.names(g(), "France") && !spain.names(g(), ""));
        assert_eq!(spain.places().len(), 1);
    }

    #[test]
    fn population_decides_the_reach() {
        assert!(city_radius_km(6_000_000) > city_radius_km(500_000));
        assert!(city_radius_km(500_000) > city_radius_km(20_000));
    }
}
