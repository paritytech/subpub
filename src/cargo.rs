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

// See https://github.com/rust-lang/crates.io/blob/d240463e8c807b3c29248dec6bd31779f49dd424/src/util/errors/json.rs#L139-L146
fn detect_rate_limit_error(err_msg: &str) -> Option<String> {
    err_msg
        .match_indices("You have published too many crates")
        .next()
        .map(|(idx, _)| err_msg.chars().skip(idx).collect())
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

/*
   By https://github.com/rust-lang/cargo/blob/1d8cdaa01eea3af5ad36c600fbf21b51a1684454/crates/crates-io/lib.rs#L281
   we can deduce that cargo currently relies on the curl library, thus we need
   to take into account any relevant errors of
   https://docs.rs/curl/latest/curl/struct.Error.html (which are themselves
   related to https://curl.se/libcurl/c/libcurl-errors.html). That being said,
   network-related errors can also happen outside of curl, such as name
   resolution errors.
*/
fn detect_spurious_network_error(err_msg: &str) -> Option<String> {
    err_msg
        .match_indices(
            "dns error: failed to lookup address information: Temporary failure in name resolution",
        )
        .next()
        .map(|(idx, _)| err_msg.chars().skip(idx).collect())
}

#[test]
fn test_detect_spurious_network_error() {
    let full_error_msg = "
Updating crates.io index
Packaging sc-rpc-api v0.10.0 (/substrate/client/rpc-api)
Uploading sc-rpc-api v0.10.0 (/substrate/client/rpc-api)
error: failed to publish crate sc-rpc-ai
Caused by:
  dns error: failed to lookup address information: Temporary failure in name resolution
";

    let expected_error_msg_part =
        "dns error: failed to lookup address information: Temporary failure in name resolution\n";

    assert_eq!(
        detect_spurious_network_error(full_error_msg),
        Some(expected_error_msg_part.to_owned())
    );
}

pub enum PublishError {
    RateLimited(String),
    SpuriousNetworkError(String),
    Any(anyhow::Error),
}

impl From<io::Error> for PublishError {
    fn from(err: io::Error) -> Self {
        Self::Any(anyhow!(err))
    }
}

const DEV_DEPS_TROUBLESHOOT_HINT: &str = "
Note: dev-dependencies are stripped before publishing. This might cause errors
during pre-publish verification in case a dev-dependency is used for a cargo
feature. If you run into errors such as:

    error: failed to parse manifest at `/path/to/Cargo.toml`
    Caused by:
      feature `bar` includes `foo/benchmarks`, but `foo` is not a dependency

Or:

    error[XXX]: unresolved import `foo::bar`

Assuming that the crate works fine locally, the error occurs because `foo` is a
dev-dependency, which was stripped before publishing. You can work around that
by using `foo` conditionally behind a feature flag or by promoting `foo` to a
normal dependency.
";

pub fn publish_crate<P: AsRef<Path>>(
    krate: &str,
    manifest_path: P,
    verify: bool,
) -> Result<(), PublishError> {
    let mut cmd = Command::new("cargo");
    cmd.arg("publish");

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
        } else if let Some(spurious_network_err) = detect_spurious_network_error(&err_msg) {
            return Err(PublishError::SpuriousNetworkError(spurious_network_err));
        } else {
            return Err(PublishError::Any(anyhow!(
                "Failed to publish crate {krate}. Command failed: {cmd:?}\nOutput:\n{}\n{}",
                err_msg,
                DEV_DEPS_TROUBLESHOOT_HINT,
            )));
        }
    }

    Ok(())
}
