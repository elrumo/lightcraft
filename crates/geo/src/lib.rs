//! Places for LightCraft, with no network: where a photo's GPS position is ("Madrid, Spain"), what
//! a place name means ("madrid" → the city, the region, the country), and the maths of a map.
//!
//! - [`Gazetteer`] — the offline place database (GeoNames cities, regions, countries; reverse and
//!   forward lookups); [`PlaceFilter`] tests many positions against a set of places.
//! - [`geodesy`] — distances and the Web Mercator projection tiles are laid out in.
//! - [`land`] — the world's coastlines as a triangle mesh (Natural Earth), drawn when no map tiles
//!   are available.
//! - [`text::normalize`] — case and accent folding shared by the name table and the search.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod codec;
pub mod gazetteer;
pub mod geodesy;
pub mod land;
pub mod text;

pub use codec::DataError;
pub use gazetteer::{City, Country, Gazetteer, Kind, LOCATE_KM, Located, MAX_NAME_WORDS, PlaceFilter, PlaceId, PlaceName, Region, city_radius_km};
pub use text::normalize;
