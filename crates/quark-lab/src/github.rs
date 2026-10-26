//! GitHub, asked what a release has to download: Lean's, elan's, TinyTeX's,
//! texlab's. A few kilobytes from its API, asked only when the person asks
//! for a plan; tests answer for it.

use std::io::Read;
use std::time::Duration;

use serde_json::Value;

/// What asks: GitHub over the network, or a test's answers.
pub trait Fetch: Send + Sync {
    /// The status and body of a GET.
    fn get(&self, url: &str) -> Result<(u16, String), String>;
}

/// Nobody: what a Lean that must never reach the network asks, in tests.
pub struct Offline;

impl Fetch for Offline {
    fn get(&self, _: &str) -> Result<(u16, String), String> {
        Err("GitHub is not asked here".into())
    }
}

/// The network, as the literature reaches it: rustls with the platform's
/// certificates, https only, a name that says who is asking.
pub struct Live {
    agent: ureq::Agent,
}

impl Default for Live {
    fn default() -> Live {
        let tls = ureq::tls::TlsConfig::builder()
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .https_only(true)
            .tls_config(tls)
            .timeout_global(Some(Duration::from_secs(20)))
            .max_redirects(5)
            .build()
            .into();
        Live { agent }
    }
}

impl Fetch for Live {
    fn get(&self, url: &str) -> Result<(u16, String), String> {
        let response = self
            .agent
            .get(url)
            .header(
                "User-Agent",
                &format!(
                    "Quark/{} (+https://github.com/razeenwasif/Quark)",
                    env!("CARGO_PKG_VERSION")
                ),
            )
            .header("Accept", "application/vnd.github+json")
            .call()
            .map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let mut body = String::new();
        response
            .into_body()
            .into_with_config()
            .limit(8 * 1024 * 1024)
            .reader()
            .read_to_string(&mut body)
            .map_err(|e| e.to_string())?;
        Ok((status, body))
    }
}

/// A release, as much of it as a plan needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub tag: String,
    pub prerelease: bool,
    pub draft: bool,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub url: String,
    /// Its SHA-256, in hex, when GitHub gives one.
    pub sha256: Option<String>,
}

fn release_of(v: &Value) -> Option<Release> {
    let assets = v["assets"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|a| {
                    Some(Asset {
                        name: a["name"].as_str()?.to_string(),
                        size: a["size"].as_u64().unwrap_or(0),
                        url: a["browser_download_url"].as_str()?.to_string(),
                        sha256: a["digest"]
                            .as_str()
                            .and_then(|d| d.strip_prefix("sha256:"))
                            .filter(|h| h.len() == 64)
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Release {
        tag: v["tag_name"].as_str()?.to_string(),
        prerelease: v["prerelease"].as_bool().unwrap_or(false),
        draft: v["draft"].as_bool().unwrap_or(false),
        assets,
    })
}

pub fn releases(json: &str) -> Result<Vec<Release>, String> {
    let v: Value = serde_json::from_str(json)
        .map_err(|e| format!("GitHub's list of releases did not read: {e}"))?;
    Ok(v.as_array()
        .map(|list| list.iter().filter_map(release_of).collect())
        .unwrap_or_default())
}

pub fn release(json: &str) -> Result<Release, String> {
    let v: Value =
        serde_json::from_str(json).map_err(|e| format!("GitHub's release did not read: {e}"))?;
    release_of(&v).ok_or_else(|| "GitHub's release named no tag".to_string())
}
