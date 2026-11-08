//! Scrapes the public web client_id from soundcloud.com's JS bundles (like cobalt does)
//! and caches it in `~/.cache/suprpull/client_id.json` keyed by `__sc_version`.

use anyhow::Result;

/// Return a valid client_id, using the cache unless `force_refresh` is set
/// or the `__sc_version` on the homepage changed.
pub async fn get_client_id(http: &reqwest::Client, force_refresh: bool) -> Result<String> {
    let _ = (http, force_refresh);
    todo!()
}

/// All `https://a-v2.sndcdn.com/assets/*.js` script URLs in page order.
pub fn extract_script_urls(html: &str) -> Vec<String> {
    let _ = html;
    todo!()
}

/// Value of `__sc_version="..."` in the homepage HTML.
pub fn extract_sc_version(html: &str) -> Option<String> {
    let _ = html;
    todo!()
}

/// Find `client_id:"<32 alnum>"` (or `client_id=<32 alnum>`) in a JS bundle.
pub fn extract_client_id(js: &str) -> Option<String> {
    let _ = js;
    todo!()
}
