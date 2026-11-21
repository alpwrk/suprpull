//! Scrapes the public web client_id from soundcloud.com's JS bundles
//! and caches it in `~/.cache/suprpull/client_id.json` keyed by `__sc_version`.

use std::path::PathBuf;
use std::sync::LazyLock;

use anyhow::{Context, Result, anyhow};
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Browser-like User-Agent used for all soundcloud.com requests.
pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

const HOMEPAGE: &str = "https://soundcloud.com/";

static SCRIPT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<script\b[^>]*?\bsrc="(https://a-v2\.sndcdn\.com/assets/[^"]+\.js)""#)
        .expect("valid script regex")
});

static VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"__sc_version\s*=\s*"(\d+)""#).expect("valid version regex"));

static CLIENT_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"client_id\s*[:=]\s*"?([A-Za-z0-9]{32})(?:[^A-Za-z0-9]|$)"#)
        .expect("valid client_id regex")
});

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CachedId {
    client_id: String,
    sc_version: Option<String>,
}

fn cache_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "suprpull").map(|d| d.cache_dir().join("client_id.json"))
}

async fn read_cache() -> Option<CachedId> {
    let path = cache_path()?;
    let data = tokio::fs::read(&path).await.ok()?;
    serde_json::from_slice(&data).ok()
}

async fn write_cache(entry: &CachedId) -> Result<()> {
    let path = cache_path().ok_or_else(|| anyhow!("no cache directory available"))?;
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    tokio::fs::write(&path, serde_json::to_vec_pretty(entry)?).await?;
    Ok(())
}

async fn fetch_text(http: &reqwest::Client, url: &str) -> Result<String> {
    let resp = http
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .with_context(|| format!("request to {url} failed"))?
        .error_for_status()
        .with_context(|| format!("request to {url} failed"))?;
    Ok(resp.text().await?)
}

/// Return a valid client_id, using the cache unless `force_refresh` is set
/// or the `__sc_version` on the homepage changed.
pub async fn get_client_id(http: &reqwest::Client, force_refresh: bool) -> Result<String> {
    let cached = read_cache().await;
    let html = fetch_text(http, HOMEPAGE)
        .await
        .context("could not load the soundcloud.com homepage")?;
    let version = extract_sc_version(&html);

    if !force_refresh
        && let Some(c) = &cached
        && c.sc_version == version
    {
        return Ok(c.client_id.clone());
    }

    let scripts = extract_script_urls(&html);
    if scripts.is_empty() {
        return Err(anyhow!(
            "no script bundles found on the soundcloud.com homepage"
        ));
    }
    for url in scripts.iter().rev() {
        let Ok(js) = fetch_text(http, url).await else {
            continue;
        };
        if let Some(id) = extract_client_id(&js) {
            let entry = CachedId {
                client_id: id.clone(),
                sc_version: version,
            };
            // A failing cache write is not fatal.
            let _ = write_cache(&entry).await;
            return Ok(id);
        }
    }
    Err(anyhow!(
        "could not find a client_id in soundcloud.com's script bundles"
    ))
}

/// All `https://a-v2.sndcdn.com/assets/*.js` script URLs in page order.
pub fn extract_script_urls(html: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for cap in SCRIPT_RE.captures_iter(html) {
        let url = cap[1].to_string();
        if !out.contains(&url) {
            out.push(url);
        }
    }
    out
}

/// Value of `__sc_version="..."` in the homepage HTML.
pub fn extract_sc_version(html: &str) -> Option<String> {
    VERSION_RE.captures(html).map(|c| c[1].to_string())
}

/// Find `client_id:"<32 alnum>"` (or `client_id=<32 alnum>`) in a JS bundle.
pub fn extract_client_id(js: &str) -> Option<String> {
    CLIENT_ID_RE.captures(js).map(|c| c[1].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HTML: &str = r#"<!DOCTYPE html><html><head>
<script>window.__sc_version="1731000123"</script>
</head><body>
<script crossorigin src="https://a-v2.sndcdn.com/assets/0-1a2b3c4d.js"></script>
<script crossorigin src="https://a-v2.sndcdn.com/assets/49-deadbeef.js"></script>
<script src="https://a-v2.sndcdn.com/assets/app-9f8e7d6c.js"></script>
<script crossorigin src="https://a-v2.sndcdn.com/assets/0-1a2b3c4d.js"></script>
<script src="https://other.example.com/assets/x.js"></script>
<script crossorigin src="https://a-v2.sndcdn.com/assets/styles.css"></script>
</body></html>"#;

    #[test]
    fn script_urls_in_order_deduped() {
        assert_eq!(
            extract_script_urls(HTML),
            vec![
                "https://a-v2.sndcdn.com/assets/0-1a2b3c4d.js",
                "https://a-v2.sndcdn.com/assets/49-deadbeef.js",
                "https://a-v2.sndcdn.com/assets/app-9f8e7d6c.js",
            ]
        );
    }

    #[test]
    fn sc_version() {
        assert_eq!(extract_sc_version(HTML).as_deref(), Some("1731000123"));
        assert_eq!(extract_sc_version("<html></html>"), None);
    }

    #[test]
    fn client_id_object_literal() {
        let js =
            r#"e.exports={env:"production",client_id:"Xy7TestClientId0000000000000000a",api:"x"}"#;
        assert_eq!(
            extract_client_id(js).as_deref(),
            Some("Xy7TestClientId0000000000000000a")
        );
    }

    #[test]
    fn client_id_query_and_spaces() {
        let js = r#"fetch("https://api-v2.soundcloud.com/x?client_id=abcdefghijABCDEFGHIJ0123456789zz&app=1")"#;
        assert_eq!(
            extract_client_id(js).as_deref(),
            Some("abcdefghijABCDEFGHIJ0123456789zz")
        );
        let js2 = r#"client_id : "ABCDEFGHIJKLMNOPQRSTUVWXYZ012345""#;
        assert_eq!(
            extract_client_id(js2).as_deref(),
            Some("ABCDEFGHIJKLMNOPQRSTUVWXYZ012345")
        );
    }

    #[test]
    fn client_id_rejects_wrong_length() {
        assert_eq!(extract_client_id(r#"client_id:"short""#), None);
        assert_eq!(
            extract_client_id(r#"client_id:"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789""#),
            None
        );
        assert_eq!(extract_client_id(r#"client_id:e.clientId"#), None);
    }
}
