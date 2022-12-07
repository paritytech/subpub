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
