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

use anyhow::{anyhow, Context};

use crate::git::git_remote_head_sha;

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
        return Err(anyhow!(
            "Non-200 status trying to connect to {url} ({})",
            res.status()
        ));
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
    let req_url = format!("{crates_api}/crates/{}/versions", name.as_ref());
    let res = client
        .get(&req_url)
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
        return Err(anyhow!(
            "Unexpected response status {} for {}",
            res_status,
            req_url,
        ));
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
                Err(anyhow!(
                    "Unexpected response status {} for {}",
                    res_status,
                    req_url
                ))
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

/// Constructs the crate's registry path as defined on
/// https://doc.rust-lang.org/cargo/reference/registries.html#index-format
/// Adapted from https://github.com/frewsxcv/rust-crates-index/blob/868d651f783fae41e79c9eee01d2679f53dd90e7/src/lib.rs#L287
fn cratesio_index_crate_path(krate: &str) -> String {
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

pub struct CratesIoIndexConfiguration<'a> {
    pub url: &'a String,
    pub repository: &'a String,
}

pub fn does_crate_exist_in_cratesio_index(
    index_conf: &CratesIoIndexConfiguration,
    krate: &str,
    version: &semver::Version,
) -> anyhow::Result<bool> {
    let head_sha = git_remote_head_sha(index_conf.repository)?;

    let req_url = get_cratesio_index_metadata_url(index_conf.url, &head_sha, krate);
    let req = reqwest::blocking::Client::new().get(&req_url);
    let res = req
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
        return Err(anyhow!(
            "Unexpected response status {} for {}",
            res_status,
            req_url
        ));
    }

    // Each line of the metadata file is a JSON object with a .vers field
    // Example: {"name":"pallet-foo","vers":"2.0.0-alpha.3","deps":[]}
    let res_data = res
        .text_with_charset("utf-8")
        .with_context(|| format!("Failed to parse response as utf-8 from {}", req_url))?;

    let target_version = version.to_string();

    #[derive(serde::Deserialize)]
    struct IndexMetadataLine {
        pub vers: String,
    }
    for line in res_data.lines().rev() {
        let line = serde_json::from_str::<IndexMetadataLine>(line)
            .with_context(|| format!("Unable to parse line as IndexMetadataLine: {}", line))?;
        if line.vers == target_version {
            return Ok(true);
        }
    }

    Ok(false)
}

fn get_cratesio_index_metadata_url(index_url: &str, head_sha: &str, krate: &str) -> String {
    let crate_path = cratesio_index_crate_path(krate);
    format!("{}/{}/{}/{}", index_url, head_sha, crate_path, krate)
}

#[test]
#[cfg(feature = "test-0")]
fn test_get_cratesio_index_url() {
    let index_url = "https://raw.githubusercontent.com/rust-lang/crates.io-index";
    let head_sha = "d90b3649f26334dc4026112ba8208993cbd88116";

    let krate = "fork-tree";
    assert_eq!(
        get_cratesio_index_metadata_url(index_url, head_sha, krate),
        format!("{}/{}/fo/rk/{}", index_url, head_sha, krate)
    );

    let krate = "sc-network";
    assert_eq!(
        get_cratesio_index_metadata_url(index_url, head_sha, krate),
        format!("{}/{}/sc/-n/{}", index_url, head_sha, krate)
    );
}
