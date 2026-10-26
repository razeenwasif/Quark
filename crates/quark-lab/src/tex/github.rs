//! What GitHub says there is to install: TinyTeX's newest release and
//! texlab's, read from its API — a few kilobytes, asked only when the person
//! asks for a plan — with the SHA-256 digest GitHub publishes for each file.

use super::{Bundle, Platform, TexDownload, TexPlan};
use crate::github::{release, Fetch};

/// TinyTeX's newest release: TeX Live, made small, a month at a time.
pub const TINYTEX_LATEST: &str =
    "https://api.github.com/repos/rstudio/tinytex-releases/releases/latest";
/// texlab's newest release: the language server the editor completes with.
pub const TEXLAB_LATEST: &str = "https://api.github.com/repos/latex-lsp/texlab/releases/latest";

/// How much bigger TinyTeX is unpacked than downloaded — an estimate: TeX's
/// files compress well under xz.
pub const TINYTEX_GROWTH: f64 = 3.5;
/// The same for texlab, one program in a gzip.
pub const TEXLAB_GROWTH: f64 = 2.5;
/// latexmk, which tlmgr installs from CTAN — an estimate.
pub const LATEXMK: u64 = 1_000_000;

/// What GitHub says when it will not answer.
fn refused(status: u16, what: &str) -> String {
    match status {
        403 | 429 => {
            "GitHub is limiting how often this address may ask it; try again in an hour".into()
        }
        _ => format!("GitHub answered {status} for {what}"),
    }
}

/// The plan: the newest TinyTeX in the bundle asked for and texlab, built
/// for this machine, and what they cost to download and keep.
pub fn plan(
    fetch: &dyn Fetch,
    platform: &Platform,
    bundle: Bundle,
    home: &str,
    free: u64,
) -> Result<TexPlan, String> {
    let (status, body) = fetch.get(TINYTEX_LATEST)?;
    if status != 200 {
        return Err(refused(status, "TinyTeX's release"));
    }
    let tinytex = release(&body)?;
    let name = format!(
        "{}-{}-{}.tar.xz",
        bundle.prefix(),
        platform.tinytex,
        tinytex.tag
    );
    let tinytex_asset = tinytex
        .assets
        .iter()
        .find(|a| a.name == name)
        .ok_or_else(|| {
            format!(
                "TinyTeX {} has no build for this machine ({name})",
                tinytex.tag
            )
        })?;

    let (status, body) = fetch.get(TEXLAB_LATEST)?;
    if status != 200 {
        return Err(refused(status, "texlab's release"));
    }
    let texlab = release(&body)?;
    let texlab_name = format!("texlab-{}.tar.gz", platform.texlab);
    let texlab_asset = texlab
        .assets
        .iter()
        .find(|a| a.name == texlab_name)
        .ok_or_else(|| {
            format!(
                "texlab {} has no build for this machine ({texlab_name})",
                texlab.tag
            )
        })?;

    let downloads = vec![
        TexDownload {
            what: format!("{} {}", bundle.prefix(), tinytex.tag),
            host: "github.com".into(),
            bytes: tinytex_asset.size,
            exact: true,
            verified: tinytex_asset.sha256.is_some(),
        },
        TexDownload {
            what: "latexmk, by TeX's package manager".into(),
            host: "ctan.org".into(),
            bytes: LATEXMK,
            exact: false,
            verified: false,
        },
        TexDownload {
            what: format!("texlab {}", texlab.tag),
            host: "github.com".into(),
            bytes: texlab_asset.size,
            exact: true,
            verified: texlab_asset.sha256.is_some(),
        },
    ];
    let unpacked = (tinytex_asset.size as f64 * TINYTEX_GROWTH) as u64
        + (texlab_asset.size as f64 * TEXLAB_GROWTH) as u64
        + LATEXMK;
    let download: u64 = downloads.iter().map(|d| d.bytes).sum();
    // At its fullest, while what was downloaded is unpacked and before it is deleted.
    let disk = download + unpacked;
    Ok(TexPlan {
        bundle,
        tinytex: tinytex.tag.clone(),
        texlab: texlab.tag.clone(),
        downloads,
        download,
        disk,
        kept: unpacked,
        free,
        fits: free > disk,
        home: home.to_string(),
        tinytex_url: tinytex_asset.url.clone(),
        tinytex_sha256: tinytex_asset.sha256.clone(),
        texlab_url: texlab_asset.url.clone(),
        texlab_sha256: texlab_asset.sha256.clone(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    pub const TINYTEX: &str = include_str!("../../fixtures/github/tinytex-latest.json");
    pub const TEXLAB: &str = include_str!("../../fixtures/github/texlab-latest.json");

    /// Answers by URL, and remembers what was asked.
    #[derive(Default)]
    pub struct Script {
        pub answers: BTreeMap<String, (u16, String)>,
        pub asked: Mutex<Vec<String>>,
    }

    impl Script {
        /// GitHub as recorded on 2026-10-12.
        pub fn recorded() -> Script {
            let mut answers = BTreeMap::new();
            answers.insert(TINYTEX_LATEST.to_string(), (200, TINYTEX.to_string()));
            answers.insert(TEXLAB_LATEST.to_string(), (200, TEXLAB.to_string()));
            Script {
                answers,
                asked: Mutex::default(),
            }
        }
    }

    impl Fetch for Script {
        fn get(&self, url: &str) -> Result<(u16, String), String> {
            self.asked.lock().unwrap().push(url.to_string());
            Ok(self
                .answers
                .get(url)
                .cloned()
                .unwrap_or((404, "404: Not Found".into())))
        }
    }

    const MAC: Platform = Platform {
        tinytex: "darwin",
        texlab: "aarch64-macos",
    };
    const LINUX: Platform = Platform {
        tinytex: "linux-x86_64",
        texlab: "x86_64-linux",
    };

    #[test]
    fn the_plan_is_tinytex_and_texlab_for_this_machine_with_their_digests_and_sizes() {
        let script = Script::recorded();
        let plan = plan(
            &script,
            &MAC,
            Bundle::Full,
            "/home/you/.quark/tex",
            80_000_000_000,
        )
        .unwrap();
        assert_eq!(
            (plan.tinytex.as_str(), plan.texlab.as_str()),
            ("v2026.10", "v5.26.0")
        );
        assert!(
            plan.tinytex_url
                .ends_with("/TinyTeX-darwin-v2026.10.tar.xz"),
            "{}",
            plan.tinytex_url
        );
        assert!(
            plan.texlab_url.ends_with("/texlab-aarch64-macos.tar.gz"),
            "{}",
            plan.texlab_url
        );
        assert!(
            plan.tinytex_sha256
                .as_deref()
                .is_some_and(|h| h.len() == 64)
                && plan.texlab_sha256.is_some()
        );
        let tinytex = &plan.downloads[0];
        assert!(
            tinytex.exact && tinytex.verified && tinytex.bytes > 200_000_000,
            "{tinytex:?}"
        );
        assert!(!plan.downloads[1].exact, "latexmk's size is an estimate");
        assert!(plan.fits && plan.disk > plan.download && plan.kept > tinytex.bytes);

        let small = super::plan(
            &script,
            &LINUX,
            Bundle::Small,
            "/home/you/.quark/tex",
            80_000_000_000,
        )
        .unwrap();
        assert!(
            small
                .tinytex_url
                .ends_with("/TinyTeX-1-linux-x86_64-v2026.10.tar.xz")
                && small.downloads[0].bytes < 100_000_000,
            "{small:?}"
        );
        let cramped =
            super::plan(&script, &MAC, Bundle::Full, "/home/you/.quark/tex", 1_000).unwrap();
        assert!(!cramped.fits);
    }

    #[test]
    fn a_platform_without_a_build_or_a_github_that_will_not_answer_says_so() {
        let odd = Platform {
            tinytex: "plan9",
            texlab: "plan9",
        };
        assert!(plan(&Script::recorded(), &odd, Bundle::Full, "/h", 1)
            .unwrap_err()
            .contains("no build for this machine"));
        let mut limited = Script::recorded();
        limited
            .answers
            .insert(TINYTEX_LATEST.into(), (403, "rate limited".into()));
        assert!(plan(&limited, &MAC, Bundle::Full, "/h", 1)
            .unwrap_err()
            .contains("try again in an hour"));
    }
}
