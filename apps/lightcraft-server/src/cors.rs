//! Cross-origin requests to the API. A web build served from another site than this server (a
//! static host, a CDN) can only call it if the server says the site may (`--cors-origin`,
//! `$LIGHTCRAFT_CORS`). Without any origin set nothing is added: a page is served by the server it
//! talks to, which needs none of this. Devices sign in with a bearer token in a header, never a
//! cookie, so allowing an origin lets that page call the API only as a user who gave it a token.
//! Only `/api/…` (not the admin page's `/api/admin/…`) answers cross-origin calls.

use crate::http::Header;

/// The sites allowed to call the API from a browser.
#[derive(Clone, Debug, Default)]
pub struct Cors {
    origins: Vec<String>,
    any: bool,
}

/// What a page may send to the API.
const ALLOW_HEADERS: &str = "Authorization, Content-Type, Range, Content-Range";
const ALLOW_METHODS: &str = "GET, HEAD, POST, PUT, DELETE, OPTIONS";
/// What a page may read from an answer (beside the safe ones).
const EXPOSE_HEADERS: &str = "X-LightCraft-Size, Upload-Offset, Content-Range, Accept-Ranges, Retry-After";

/// An origin as browsers send it: scheme and host (and port), lower case, no path.
fn normalize(o: &str) -> String {
    o.trim().trim_end_matches('/').to_ascii_lowercase()
}

impl Cors {
    /// The origins allowed (`https://photos.example.com`; `*`: any site).
    pub fn new<S: AsRef<str>>(origins: &[S]) -> Cors {
        let mut c = Cors::default();
        for o in origins {
            let o = normalize(o.as_ref());
            if o == "*" {
                c.any = true;
            } else if !o.is_empty() && !c.origins.contains(&o) {
                c.origins.push(o);
            }
        }
        c
    }

    /// Is any origin allowed?
    pub fn is_on(&self) -> bool {
        self.any || !self.origins.is_empty()
    }

    /// May a page from `origin` (an `Origin` header) call the API?
    pub fn allows(&self, origin: &str) -> bool {
        self.any || self.origins.contains(&normalize(origin))
    }

    /// The headers an answer to `origin` carries (empty when the origin isn't allowed).
    pub fn answer_headers(&self, origin: &str) -> Vec<Header> {
        if !self.allows(origin) {
            return Vec::new();
        }
        // (an origin is echoed, never a wildcard: the answer then depends on it)
        [("Access-Control-Allow-Origin", origin.trim()), ("Vary", "Origin"), ("Access-Control-Expose-Headers", EXPOSE_HEADERS)]
            .into_iter()
            .filter_map(|(k, v)| Header::from_bytes(k.as_bytes(), v.as_bytes()).ok())
            .collect()
    }

    /// The headers of the answer to a preflight (`OPTIONS`) from `origin`.
    pub fn preflight_headers(&self, origin: &str) -> Vec<Header> {
        let mut h = self.answer_headers(origin);
        if !h.is_empty() {
            h.extend(
                [("Access-Control-Allow-Methods", ALLOW_METHODS), ("Access-Control-Allow-Headers", ALLOW_HEADERS), ("Access-Control-Max-Age", "600")]
                    .into_iter()
                    .filter_map(|(k, v)| Header::from_bytes(k.as_bytes(), v.as_bytes()).ok()),
            );
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(h: &[Header]) -> Vec<String> {
        h.iter().map(|h| h.field.to_string().to_ascii_lowercase()).collect()
    }

    #[test]
    fn nothing_is_allowed_by_default() {
        let c = Cors::new::<&str>(&[]);
        assert!(!c.is_on() && !c.allows("https://photos.example.com"));
        assert!(c.answer_headers("https://photos.example.com").is_empty());
    }

    #[test]
    fn listed_origins_are_allowed_however_they_are_written() {
        let c = Cors::new(&["https://Photos.Example.com/", "http://localhost:5173", " ", "https://photos.example.com"]);
        assert!(c.is_on());
        assert!(c.allows("https://photos.example.com") && c.allows("HTTPS://PHOTOS.EXAMPLE.COM") && c.allows("http://localhost:5173"));
        assert!(!c.allows("https://evil.example.com") && !c.allows("https://photos.example.com.evil.com") && !c.allows("http://photos.example.com"));
        assert!(!c.allows("null") && !c.allows(""));
        assert_eq!(c.origins.len(), 2, "listed once: {:?}", c.origins);
    }

    #[test]
    fn a_wildcard_allows_any_site_and_echoes_it() {
        let c = Cors::new(&["*"]);
        assert!(c.allows("https://anywhere.example"));
        let h = c.answer_headers("https://anywhere.example");
        assert!(h.iter().any(|h| h.field.equiv("access-control-allow-origin") && h.value == "https://anywhere.example"));
    }

    #[test]
    fn preflight_says_what_a_page_may_send() {
        let c = Cors::new(&["https://photos.example.com"]);
        let h = c.preflight_headers("https://photos.example.com");
        let n = names(&h);
        for want in ["access-control-allow-origin", "vary", "access-control-allow-methods", "access-control-allow-headers", "access-control-max-age"]
        {
            assert!(n.contains(&want.to_string()), "{want} in {n:?}");
        }
        let allow = h.iter().find(|h| h.field.equiv("access-control-allow-headers")).map(|h| h.value.clone()).unwrap_or_default();
        assert!(allow.contains("Authorization") && allow.contains("Content-Range"), "{allow}");
        assert!(c.preflight_headers("https://evil.example.com").is_empty(), "no permission for others");
    }

    #[test]
    fn answers_expose_what_the_client_reads() {
        let c = Cors::new(&["https://photos.example.com"]);
        let h = c.answer_headers("https://photos.example.com");
        let ex = h.iter().find(|h| h.field.equiv("access-control-expose-headers")).map(|h| h.value.clone()).unwrap_or_default();
        for want in ["Upload-Offset", "X-LightCraft-Size", "Content-Range"] {
            assert!(ex.contains(want), "{want} in {ex}");
        }
    }

    #[test]
    fn a_hostile_origin_header_is_not_echoed() {
        let c = Cors::new(&["*"]);
        // (headers with line breaks can't be built at all)
        assert!(c.answer_headers("https://a.example\r\nSet-Cookie: x=1").iter().all(|h| !h.value.contains('\n')));
    }
}
