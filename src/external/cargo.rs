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

use std::{env, io, path::Path, process::Command};

use anyhow::anyhow;

pub enum PublishError {
    RateLimited(String),
    Any(anyhow::Error),
}

impl From<io::Error> for PublishError {
    fn from(err: io::Error) -> Self {
        Self::Any(anyhow!(err))
    }
}

// See https://github.com/rust-lang/crates.io/blob/d240463e8c807b3c29248dec6bd31779f49dd424/src/util/errors/json.rs#L139-L146
fn detect_rate_limit_error(err_msg: &str) -> Option<String> {
    err_msg
        .match_indices("You have published too many crates")
        .next()
        .map(|(idx, _)| err_msg.chars().skip(idx).collect())
}

pub fn publish_crate<P: AsRef<Path>>(
    krate: &str,
    manifest_path: P,
    verify: bool,
) -> Result<(), PublishError> {
    let mut cmd = Command::new("cargo");
    cmd.arg("publish").arg("-v");

    if let Ok(registry) = env::var("SPUB_REGISTRY") {
        cmd.env("CARGO_REGISTRY_DEFAULT", &registry)
            .arg("--registry")
            .arg(registry)
            .arg("--token")
            .arg(env::var("SPUB_REGISTRY_TOKEN").unwrap());
    }

    if !verify {
        cmd.arg("--no-verify");
    }

    let output = cmd
        .arg("--allow-dirty")
        .arg("--manifest-path")
        .arg(manifest_path.as_ref())
        .output()?;
    if !output.status.success() {
        // Ideally we'd detect rate limiting problems by the exit code, but
        // cargo's exit code isn't fine-grained, so do it by the error message
        // instead.
        let err_msg = String::from_utf8_lossy(&output.stderr[..]);
        if let Some(rate_limit_err) = detect_rate_limit_error(&err_msg) {
            return Err(PublishError::RateLimited(rate_limit_err));
        } else {
            return Err(PublishError::Any(anyhow!(
                "Failed to publish crate {krate}. Command failed: {cmd:?}. Output:\n{}",
                err_msg
            )));
        }
    }

    Ok(())
}

#[test]
fn test_detect_rate_limit_error() {
    let full_error_msg = "
Updating crates.io index
Packaging sc-rpc-api v0.10.0 (/substrate/client/rpc-api)
Uploading sc-rpc-api v0.10.0 (/substrate/client/rpc-api)
You have published too many crates in a short period of time. Please try again after {PLACEHOLDER} or email help@crates.io to have your limit increased.
";

    let expected_error_msg_part = "You have published too many crates in a short period of time. Please try again after {PLACEHOLDER} or email help@crates.io to have your limit increased.\n";

    assert_eq!(
        detect_rate_limit_error(full_error_msg),
        Some(expected_error_msg_part.to_owned())
    );
}
