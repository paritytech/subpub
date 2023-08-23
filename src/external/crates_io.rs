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
use serde::Deserialize;
use std::collections::HashSet;

const CRATES_API: &str = "https://crates.io/api/v1";

pub fn does_crate_exist(name: &str, version: &semver::Version) -> anyhow::Result<bool> {
	let client = reqwest::blocking::Client::new();
	let url = format!("{CRATES_API}/crates/{name}/{version}");
	let res = client
		.get(&url)
		.header(
			"User-Agent",
			"Called from https://github.com/paritytech/subpub for comparing published source against repo source",
		)
		.send()
		.with_context(|| format!("Cannot download {name}"))?;

	if !res.status().is_success() {
		// We get a 200 back even if we ask for crates/versions that don't exist,
		// so a non-200 means something worse went wrong.
		anyhow::bail!("Non-200 status trying to connect to {url} ({})", res.status());
	}

	#[allow(unused)]
	#[derive(serde::Deserialize)]
	struct SuccessfulResponse {
		version: SuccessfulResponseVersion,
	}
	#[allow(unused)]
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

/// Download a crate from crates.io.
pub fn try_download_crate(
	name: &str,
	version: &semver::Version,
) -> anyhow::Result<Option<Vec<u8>>> {
	let client = reqwest::blocking::Client::new();
	let version = version.to_string();
	let res = client
		.get(format!("{CRATES_API}/crates/{name}/{version}/download"))
		.header(
			"User-Agent",
			"Called from https://github.com/paritytech/subpub for comparing published source against repo source",
		)
		.send()
		.with_context(|| format!("Cannot download {name}"))?;

	if !res.status().is_success() {
		return Ok(None)
	}

	Ok(Some(res.bytes()?.to_vec()))
}

/// Which versions of this crate exist on crates.io?
pub fn get_known_crate_versions(name: &str) -> anyhow::Result<HashSet<semver::Version>> {
	#[derive(Deserialize)]
	struct Response {
		versions: Vec<VersionInfo>,
	}
	#[derive(Deserialize)]
	struct VersionInfo {
		num: String,
	}

	let client = reqwest::blocking::Client::new();
	let res = client
		.get(format!("{CRATES_API}/crates/{name}"))
		.header(
			"User-Agent",
			"Called from https://github.com/paritytech/subpub for checking crate versions",
		)
		.send()
		.with_context(|| format!("Cannot get details for {name}"))?;

	if !res.status().is_success() {
		anyhow::bail!("Non-200 response code getting details for {name}");
	}

	let response: Response = res.json()?;
	response
		.versions
		.into_iter()
		.map(|v| {
			semver::Version::parse(&v.num).with_context(|| "Cannot parse response into Version")
		})
		.collect()
}
