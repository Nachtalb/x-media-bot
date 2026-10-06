//! FxTwitter API client and X/Twitter link parsing.

use anyhow::{Context, Result};
use reqwest::Url;
use serde::Deserialize;

const HOSTS: &[&str] = &[
    "x.com",
    "twitter.com",
    "fxtwitter.com",
    "fixupx.com",
    "vxtwitter.com",
    "fixvx.com",
    "twittpr.com",
];

/// Post id from an X/Twitter (or fixer-site) status link.
pub fn status_id(link: &str) -> Option<u64> {
    let link = if link.contains("://") {
        link.to_owned()
    } else {
        format!("https://{link}")
    };
    let url = Url::parse(&link).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    if !HOSTS
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")))
    {
        return None;
    }
    let mut segs = url.path_segments()?;
    segs.find(|s| *s == "status" || *s == "statuses")?;
    segs.next()?.parse().ok()
}

#[derive(Debug, Deserialize)]
struct Response {
    tweet: Option<Tweet>,
}

#[derive(Debug, Deserialize)]
pub struct Tweet {
    pub url: String,
    #[serde(default)]
    pub text: String,
    pub author: Author,
    pub media: Option<Media>,
    pub quote: Option<Box<Tweet>>,
}

#[derive(Debug, Deserialize)]
pub struct Author {
    pub name: String,
    pub screen_name: String,
}

#[derive(Debug, Deserialize)]
pub struct Media {
    #[serde(default)]
    pub all: Vec<Item>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Item {
    /// "photo", "video" or "gif".
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(default)]
    pub width: u16,
    #[serde(default)]
    pub height: u16,
    #[serde(default)]
    pub duration: f64,
    #[serde(default)]
    pub formats: Vec<Format>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Format {
    pub url: String,
    #[serde(default)]
    pub container: String,
    #[serde(default)]
    pub bitrate: u64,
}

impl Tweet {
    /// The post's own media, or the quoted post's if it has none.
    pub fn media(&self) -> Vec<Item> {
        let own = self
            .media
            .as_ref()
            .map(|m| m.all.clone())
            .unwrap_or_default();
        if own.is_empty() {
            self.quote.as_ref().map(|q| q.media()).unwrap_or_default()
        } else {
            own
        }
    }
}

impl Item {
    pub fn secs(&self) -> u16 {
        self.duration.round() as u16
    }

    /// mp4 urls, best bitrate first (falls back to `url`).
    pub fn mp4_urls(&self) -> Vec<String> {
        let mut f: Vec<&Format> = self
            .formats
            .iter()
            .filter(|f| f.container == "mp4")
            .collect();
        f.sort_by_key(|f| std::cmp::Reverse(f.bitrate));
        let mut urls: Vec<String> = f.into_iter().map(|f| f.url.clone()).collect();
        if urls.is_empty() {
            urls.push(self.url.clone());
        }
        urls
    }
}

pub async fn fetch(http: &reqwest::Client, id: u64) -> Result<Option<Tweet>> {
    let resp = http
        .get(format!("https://api.fxtwitter.com/status/{id}"))
        .send()
        .await
        .context("FxTwitter request")?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let resp: Response = resp
        .error_for_status()?
        .json()
        .await
        .context("FxTwitter json")?;
    Ok(resp.tweet)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_links() {
        assert_eq!(
            status_id("https://x.com/Gnomet0shi/status/2107177216920727781"),
            Some(2107177216920727781)
        );
        assert_eq!(
            status_id("https://twitter.com/a/status/123?s=20"),
            Some(123)
        );
        assert_eq!(
            status_id("https://mobile.twitter.com/a/status/123/photo/1"),
            Some(123)
        );
        assert_eq!(status_id("https://d.fxtwitter.com/a/status/123"), Some(123));
        assert_eq!(status_id("https://x.com/i/status/123"), Some(123));
        assert_eq!(status_id("x.com/a/status/9"), Some(9));
        assert_eq!(status_id("https://x.com/a"), None);
        assert_eq!(status_id("https://notx.com/a/status/1"), None);
        assert_eq!(status_id("https://evilx.com/a/status/1"), None);
        assert_eq!(status_id("https://x.com/a/status/abc"), None);
    }

    #[test]
    fn mp4_best_first_and_quote_fallback() {
        let json = r#"{"url":"u","text":"t","author":{"name":"n","screen_name":"s"},
            "media":{"all":[]},
            "quote":{"url":"q","author":{"name":"n","screen_name":"s"},
              "media":{"all":[{"type":"video","url":"best","formats":[
                {"url":"m3u8","container":"m3u8"},
                {"url":"low","container":"mp4","bitrate":1},
                {"url":"high","container":"mp4","bitrate":9}]}]}}}"#;
        let t: Tweet = serde_json::from_str(json).unwrap();
        let m = t.media();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].mp4_urls(), vec!["high", "low"]);
    }
}
