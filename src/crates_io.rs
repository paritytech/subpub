use serde::Deserialize;
use std::collections::HashSet;
use anyhow::Context;

const CRATES_API: &str = "https://crates.io/api/v1";

/// Download a crate from crates.io.
pub fn try_download_crate(name: &str, version: &semver::Version) -> anyhow::Result<Option<Vec<u8>>> {
    let client = reqwest::blocking::Client::new();
    let version = version.to_string();
    let res = client.get(format!("{CRATES_API}/crates/{name}/{version}/download"))
        .header("User-Agent", "Called from https://github.com/paritytech/subpub for comparing published source against repo source")
        .send()
        .with_context(|| format!("Cannot download {name}"))?;

    if !res.status().is_success() {
        return Ok(None);
    }

    Ok(Some(res.bytes()?.to_vec()))
}

/// Which versions of this crate exist on crates.io?
pub fn get_known_crate_versions(name: &str) -> anyhow::Result<HashSet<semver::Version>> {
    #[derive(Deserialize)]
    struct Response {
        versions: Vec<VersionInfo>
    }
    #[derive(Deserialize)]
    struct VersionInfo {
        num: String
    }

    let client = reqwest::blocking::Client::new();
    let res = client.get(format!("{CRATES_API}/crates/{name}"))
        .header("User-Agent", "Called from https://github.com/paritytech/subpub for checking crate versions")
        .send()
        .with_context(|| format!("Cannot get details for {name}"))?;

    if !res.status().is_success() {
        anyhow::bail!("Non-200 response code getting details for {name}");
    }

    let response: Response = res.json()?;
    response
        .versions
        .into_iter()
        .map(|v|
                semver::Version::parse(&v.num)
                    .with_context(|| "Cannot parse response into Version")
        )
        .collect()
}

