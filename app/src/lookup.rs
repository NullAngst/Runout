//! Tracklist lookup on MusicBrainz. The only network access in the app, and it
//! only happens when the Look up tracklist button is pressed.

use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

const UA: &str = concat!("Runout/", env!("CARGO_PKG_VERSION"), " ( https://github.com/NullAngst/Runout )");

#[derive(Serialize)]
pub struct ReleaseHit {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub date: String,
    pub country: String,
    pub format: String,
    pub tracks: u32,
}

#[derive(Serialize)]
pub struct LookupTrack {
    /// "A1", "B3" or "4"
    pub number: String,
    pub title: String,
    pub length_s: Option<f64>,
    /// Side letter if the number has one
    pub side: Option<String>,
}

#[derive(Serialize)]
pub struct Release {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub date: String,
    pub genre: String,
    pub tracks: Vec<LookupTrack>,
}

fn enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

fn get(url: &str) -> Result<Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))).build().into();
    let mut r = agent.get(url).header("User-Agent", UA).header("Accept", "application/json").call().map_err(|e| format!("MusicBrainz: {e}"))?;
    r.body_mut().read_json::<Value>().map_err(|e| format!("MusicBrainz: {e}"))
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn credit(v: &Value) -> String {
    v.get("artist-credit")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().map(|c| format!("{}{}", s(&c["name"]), s(&c["joinphrase"]))).collect::<String>())
        .unwrap_or_default()
}

fn quote(x: &str) -> String {
    x.replace('"', "")
}

/// Search releases. `format` (the medium's MusicBrainz format: Vinyl,
/// Cassette, DAT, ...) narrows the results to that format; empty searches all.
pub fn search(artist: &str, album: &str, format: &str) -> Result<Vec<ReleaseHit>, String> {
    let mut q = Vec::new();
    if !artist.trim().is_empty() {
        q.push(format!("artist:\"{}\"", quote(artist.trim())));
    }
    if !album.trim().is_empty() {
        q.push(format!("release:\"{}\"", quote(album.trim())));
    }
    if q.is_empty() {
        return Err("enter an artist or album to search for".into());
    }
    if !format.trim().is_empty() {
        q.push(format!("format:\"{}\"", quote(format.trim())));
    }
    let url = format!("https://musicbrainz.org/ws/2/release/?query={}&fmt=json&limit=15", enc(&q.join(" AND ")));
    let v = get(&url)?;
    let mut out = Vec::new();
    for r in v["releases"].as_array().cloned().unwrap_or_default() {
        let formats: Vec<String> = r["media"].as_array().map(|m| m.iter().map(|x| s(&x["format"])).filter(|f| !f.is_empty()).collect()).unwrap_or_default();
        out.push(ReleaseHit {
            id: s(&r["id"]),
            title: s(&r["title"]),
            artist: credit(&r),
            date: s(&r["date"]),
            country: s(&r["country"]),
            format: formats.join(" + "),
            tracks: r["track-count"].as_u64().unwrap_or(0) as u32,
        });
    }
    // the medium's format only; MusicBrainz names tapes "Cassette", reels
    // "Reel-to-reel", and so on, so a plain substring match fits
    if !format.trim().is_empty() {
        let f = format.trim().to_lowercase();
        out.retain(|h| h.format.to_lowercase().contains(&f));
    }
    Ok(out)
}

pub fn release(id: &str) -> Result<Release, String> {
    if !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err("bad release id".into());
    }
    let url = format!("https://musicbrainz.org/ws/2/release/{id}?inc=recordings+artist-credits+genres&fmt=json");
    let v = get(&url)?;
    let mut tracks = Vec::new();
    for m in v["media"].as_array().cloned().unwrap_or_default() {
        for t in m["tracks"].as_array().cloned().unwrap_or_default() {
            let number = s(&t["number"]);
            let side = number.chars().next().filter(|c| c.is_ascii_alphabetic()).map(|c| c.to_ascii_uppercase().to_string());
            tracks.push(LookupTrack {
                title: s(&t["title"]),
                length_s: t["length"].as_f64().map(|ms| ms / 1000.0),
                side,
                number,
            });
        }
    }
    let genre = v["genres"].as_array().and_then(|g| g.first()).map(|g| s(&g["name"])).unwrap_or_default();
    Ok(Release { id: s(&v["id"]), title: s(&v["title"]), artist: credit(&v), date: s(&v["date"]).chars().take(4).collect(), genre, tracks })
}
