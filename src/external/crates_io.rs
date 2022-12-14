// Copyright 2019-2022 Parity Technologies (UK) Ltd.
// This file is part of subpub.
//
// subpub is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// subpub is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with subpub.  If not, see <http://www.gnu.org/licenses/>.

use std::env;

use anyhow::Context;
use tracing::info;

pub fn does_crate_exist(name: &str, version: &semver::Version) -> anyhow::Result<bool> {
    let client = reqwest::blocking::Client::new();
    let crates_api = env::var("SPUB_CRATES_API").unwrap();
    let url = format!("{crates_api}/crates/{name}/{version}");
    let res = client
        .get(&url)
        .header(
            "User-Agent",
            "https://github.com/paritytech/subpub / ? : checking if the crate exists",
        )
        .send()
        .with_context(|| format!("Cannot download {name}"))?;

    let res_status = res.status();
    if res_status == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }

    if !res_status.is_success() {
        // We get a 200 back even if we ask for crates/versions that don't exist,
        // so a non-200 means something worse went wrong.
        anyhow::bail!(
            "Non-200 status trying to connect to {url} ({})",
            res.status()
        );
    }

    Ok(true)
}

pub struct CratesIoCrateVersion {
    pub version: semver::Version,
    pub yanked: bool,
}

pub fn crate_versions<Name: AsRef<str>>(name: Name) -> anyhow::Result<Vec<CratesIoCrateVersion>> {
    let client = reqwest::blocking::Client::new();
    let crates_api = env::var("SPUB_CRATES_API").unwrap();
    let url = format!("{crates_api}/crates/{}/versions", name.as_ref());
    let res = client
        .get(&url)
        .header(
            "User-Agent",
            "https://github.com/paritytech/subpub / ? : checking previous crate versions",
        )
        .send()
        .with_context(|| format!("Cannot download {}", name.as_ref()))?;

    let res_status = res.status();
    if res_status == reqwest::StatusCode::NOT_FOUND {
        return Ok(vec![]);
    }

    if !res_status.is_success() {
        anyhow::bail!("Non-200 status from response of {url} ({})", res.status());
    }

    #[derive(serde::Deserialize)]
    struct ResponseVersion {
        pub num: String,
        pub yanked: bool,
    }
    #[derive(serde::Deserialize)]
    struct Response {
        pub versions: Vec<ResponseVersion>,
    }
    res.json::<Response>()?
        .versions
        .into_iter()
        .map(|response_version| -> anyhow::Result<CratesIoCrateVersion> {
            Ok(CratesIoCrateVersion {
                version: semver::Version::parse(&response_version.num).with_context(|| {
                    format!(
                        "Failed to parse {} as semver::Version",
                        response_version.num,
                    )
                })?,
                yanked: response_version.yanked,
            })
        })
        .collect()
}

#[cfg(not(test))]
pub fn download_crate(name: &str, version: &semver::Version) -> anyhow::Result<Option<Vec<u8>>> {
    let client = reqwest::blocking::Client::new();
    let version = version.to_string();
    let crates_api = env::var("SPUB_CRATES_API").unwrap();

    let req_url = format!("{crates_api}/crates/{name}/{version}/download");
    let res = client.get(&req_url)
        .header("User-Agent", "https://github.com/paritytech/subpub / ? : comparing local crate against the published crate")
        .send()
        .with_context(|| format!("Failed to download {name} from {req_url}"))?;

    let res_status = res.status();
    match res_status {
        reqwest::StatusCode::NOT_FOUND => Ok(None),
        _ => {
            if res.status().is_success() {
                Ok(Some(res.bytes()?.to_vec()))
            } else {
                anyhow::bail!("Request to {req_url} failed with HTTP status code {res_status}");
            }
        }
    }
}

#[cfg(test)]
pub fn download_crate_for_testing(
    _: &str,
    _: &semver::Version,
    #[allow(unused_variables)] pkg_bytes: &[u8],
) -> anyhow::Result<Option<Vec<u8>>> {
    #[cfg(feature = "test-1")]
    {
        return Ok(Some(pkg_bytes.to_vec()));
    }
    #[cfg(feature = "test-2")]
    {
        return Ok(Some(vec![]));
    }
    #[cfg(feature = "test-3")]
    {
        return Ok(None);
    }
    #[allow(unreachable_code)]
    {
        Err(anyhow::anyhow!(
            "download_crate_for_testing is not set up for this test suite"
        ))
    }
}

// Adapted from https://github.com/frewsxcv/rust-crates-index/blob/868d651f783fae41e79c9eee01d2679f53dd90e7/src/lib.rs#L287
fn cratesio_index_prefix(krate: &str) -> String {
    let mut buf = String::new();

    match krate.len() {
        0 => (),
        1 => buf.push('1'),
        2 => buf.push('2'),
        3 => {
            buf.push('3');
            buf.push('/');
            if let Some(bytes) = krate.as_bytes().get(0..1) {
                for byte in bytes.to_ascii_lowercase() {
                    buf.push(byte as char);
                }
            }
        }
        _ => {
            if let Some(bytes) = krate.as_bytes().get(0..2) {
                for byte in bytes.to_ascii_lowercase() {
                    buf.push(byte as char);
                }
                buf.push('/');
                if let Some(bytes) = krate.as_bytes().get(2..4) {
                    for byte in bytes.to_ascii_lowercase() {
                        buf.push(byte as char);
                    }
                }
            }
        }
    };

    buf
}

pub fn does_crate_exist_in_cratesio_index(
    index_url: &str,
    krate: &str,
    version: &semver::Version,
) -> anyhow::Result<bool> {
    let req_url = get_cratesio_index_url(index_url, krate);
    let client = reqwest::blocking::Client::new();

    info!("Querying crate {krate} from {req_url}");
    let res = client
        .get(&req_url)
        .header(
            "User-Agent",
            "https://github.com/paritytech/subpub / ? : checking if crate is available",
        )
        .send()
        .with_context(|| format!("Failed to check {krate} from {req_url}"))?;

    let res_status = res.status();
    if res_status == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    } else if !res_status.is_success() {
        anyhow::bail!("Unexpected response status {} for {}", res_status, req_url);
    }

    let content = {
        let mut content = res
            .text_with_charset("utf-8")
            .with_context(|| format!("Failed to parse response as utf-8 from {}", req_url))?;
        if !content.ends_with('\n') {
            content.push('\n');
        }
        content
    };

    let target_version = version.to_string();

    #[derive(serde::Deserialize)]
    struct IndexMetadataLine {
        pub vers: String,
    }
    for line in content.lines().rev() {
        info!("Queried crate {krate} line: {}", line);
        let line = serde_json::from_str::<IndexMetadataLine>(line)
            .with_context(|| format!("Unable to parse line as IndexMetadataLine: {}", line))?;
        if line.vers == target_version {
            return Ok(true);
        }
    }

    Ok(false)
}

fn get_cratesio_index_url(index_url: &str, krate: &str) -> String {
    let crate_prefix = cratesio_index_prefix(krate);
    format!("{}/master/{}/{}", index_url, crate_prefix, krate)
}

#[test]
#[cfg(feature = "test-0")]
fn test_get_cratesio_index_url() {
    let index_url = "https://raw.githubusercontent.com/rust-lang/crates.io-index";

    assert_eq!(
        get_cratesio_index_url(index_url, "fork-tree"),
        format!("{}/master/fo/rk/fork-tree", index_url)
    );

    assert_eq!(
        get_cratesio_index_url(index_url, "sc-network"),
        format!("{}/master/sc/-n/sc-network", index_url)
    );
}
