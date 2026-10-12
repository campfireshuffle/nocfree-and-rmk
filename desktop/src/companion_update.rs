//! Check the public Companion release without interrupting device work.
use std::{cmp::Ordering, io::Read, time::Duration};

use semver::Version;
use serde::Deserialize;

pub(crate) const REPOSITORY_URL: &str = "https://github.com/campfireshuffle/nocfree-and-rmk";
pub(crate) const DOWNLOAD_URL: &str =
    "https://github.com/campfireshuffle/nocfree-and-rmk/releases/latest";
const RELEASE_API: &str = "https://api.github.com/repos/campfireshuffle/nocfree-and-rmk/releases/latest";
const MAX_RESPONSE_BYTES: u64 = 256 * 1024;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}

fn newer_release(response: &[u8], installed: &str) -> Option<Version> {
    let release: Release = serde_json::from_slice(response).ok()?;
    if release.draft || release.prerelease {
        return None;
    }
    let version = Version::parse(release.tag_name.strip_prefix('v')?).ok()?;
    let installed = Version::parse(installed).ok()?;
    (version.pre.is_empty() && version.cmp_precedence(&installed) == Ordering::Greater)
        .then_some(version)
}

pub(crate) fn check() -> Option<Version> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("NocFree-Companion/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let response = client
        .get(RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .ok()?
        .error_for_status()
        .ok()?;
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return None;
    }
    newer_release(&bytes, env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(tag: &str, draft: bool, prerelease: bool) -> Vec<u8> {
        serde_json::json!({"tag_name": tag, "draft": draft, "prerelease": prerelease})
            .to_string()
            .into_bytes()
    }

    #[test]
    fn uses_semantic_version_order_and_ignores_build_metadata() {
        assert_eq!(
            newer_release(&response("v0.1.10", false, false), "0.1.9")
                .unwrap()
                .to_string(),
            "0.1.10"
        );
        for tag in ["v0.1.9", "v0.1.8", "v0.1.9+rebuild"] {
            assert!(newer_release(&response(tag, false, false), "0.1.9").is_none());
        }
    }

    #[test]
    fn excludes_drafts_prereleases_and_non_app_tags() {
        for (tag, draft, prerelease) in [
            ("v0.2.0", true, false),
            ("v0.2.0", false, true),
            ("v0.2.0-beta.1", false, false),
            ("firmware-0.2.0", false, false),
            ("not-a-version", false, false),
        ] {
            assert!(newer_release(&response(tag, draft, prerelease), "0.1.9").is_none());
        }
    }

    #[test]
    fn invalid_responses_do_not_show_an_update() {
        for bytes in [b"{}".as_slice(), b"bad gateway", b"[]"] {
            assert!(newer_release(bytes, "0.1.9").is_none());
        }
        assert!(newer_release(&response("v0.2.0", false, false), "invalid").is_none());
    }
}
