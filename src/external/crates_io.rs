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

use anyhow::Context;

pub fn does_crate_exist(name: &str, version: &semver::Version) -> anyhow::Result<bool> {
    let client = reqwest::blocking::Client::new();
    let crates_api = std::env::var("SPUB_CRATES_API").unwrap();
    let url = format!("{crates_api}/crates/{name}/{version}");
    let res = client.get(&url)
        .header("User-Agent", "Called from https://github.com/paritytech/subpub for comparing local crate against published crate")
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

    #[derive(serde::Deserialize)]
    struct SuccessfulResponse {
        version: SuccessfulResponseVersion,
    }
    #[derive(serde::Deserialize)]
    struct SuccessfulResponseVersion {
        num: String,
    }

    // If the JSON response body looks like a successful one, we found
    // that crate, else we did not.
    if let Err(_e) = res.json::<SuccessfulResponse>() {
        Ok(false)
    } else {
        Ok(true)
    }
}

pub fn get_crate_versions(name: &str) -> anyhow::Result<Vec<semver::Version>> {
    let client = reqwest::blocking::Client::new();
    let crates_api = std::env::var("SPUB_CRATES_API").unwrap();
    let url = format!("{crates_api}/crates/{name}/versions");
    let res = client.get(&url)
        .header("User-Agent", "Called from https://github.com/paritytech/subpub for comparing local crate against published crate")
        .send()
        .with_context(|| format!("Cannot download {name}"))?;

    let res_status = res.status();
    if res_status == reqwest::StatusCode::NOT_FOUND {
        return Ok(vec![]);
    }

    if !res_status.is_success() {
        // We get a 200 back even if we ask for crates/versions that don't exist,
        // so a non-200 means something worse went wrong.
        anyhow::bail!(
            "Non-200 status trying to connect to {url} ({})",
            res.status()
        );
    }

    #[derive(serde::Deserialize)]
    struct SuccessfulResponseVersion {
        pub num: String,
    }
    #[derive(serde::Deserialize)]
    struct SuccessfulResponse {
        pub versions: Vec<SuccessfulResponseVersion>,
    }
    res.json::<SuccessfulResponse>()?
        .versions
        .into_iter()
        .map(|version| -> anyhow::Result<semver::Version> {
            semver::Version::parse(&version.num)
                .with_context(|| format!("Failed to parse {} as semver::Version", version.num,))
        })
        .collect()
}

/// Download a crate from crates.io.
pub fn try_download_crate(
    name: &str,
    version: &semver::Version,
) -> anyhow::Result<Option<Vec<u8>>> {
    let client = reqwest::blocking::Client::new();
    let version = version.to_string();
    let crates_api = std::env::var("SPUB_CRATES_API").unwrap();

    let req_url = format!("{crates_api}/crates/{name}/{version}/download");
    let res = client.get(&req_url)
        .header("User-Agent", "Called from https://github.com/paritytech/subpub for comparing local crate against published crate")
        .send()
        .with_context(|| format!("Cannot download {name}"))?;

    let res_status = res.status();
    if res_status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    } else if !res.status().is_success() {
        anyhow::bail!("Request to {req_url} failed with HTTP status code {res_status}");
    }

    Ok(Some(res.bytes()?.to_vec()))
}
