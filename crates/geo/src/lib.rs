//! Places for LightCraft, with no network: where a photo's GPS position is ("Madrid, Spain"), what
//! a place name means ("madrid" → the city, the region, the country), and the maths of a map.
//!
//! - [`Gazetteer`] — the offline place database (GeoNames cities, regions, countries; reverse and
//!   forward lookups); [`PlaceFilter`] tests many positions against a set of places.
//! - [`geodesy`] — distances and the Web Mercator projection tiles are laid out in.
//! - [`tiles`] — slippy-map tile coordinates and the map viewport (pan, zoom, fit); [`cluster`] — photo
//!   positions grouped into markers.
//! - [`land`] — the world's coastlines as a triangle mesh (Natural Earth), drawn when no map tiles
//!   are available.
//! - [`text::normalize`] — case and accent folding shared by the name table and the search.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod cluster;
mod codec;
pub mod gazetteer;
pub mod geodesy;
pub mod land;
pub mod text;
pub mod tiles;

pub use codec::DataError;
pub use gazetteer::{City, Country, Gazetteer, Kind, LOCATE_KM, Located, MAX_NAME_WORDS, PlaceFilter, PlaceId, PlaceName, Region, city_radius_km};
pub use text::normalize;

/// Parse the place database and the coastlines on a background thread, so the first search or the
/// first look at the Map doesn't wait for them (a tenth of a second or more). Does nothing in the
/// browser, which has no threads to spare.
pub fn preload() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = std::thread::Builder::new().name("lc-geo".into()).spawn(|| {
            let _ = Gazetteer::global();
            let _ = land::Land::global();
        });
    }
}
