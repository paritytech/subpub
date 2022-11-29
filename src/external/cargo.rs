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

use std::path::Path;
use std::process::Command;

pub fn publish_crate(root: &Path, krate: &str, verify: bool) -> anyhow::Result<()> {
    let mut cmd = Command::new("cargo");

    cmd.current_dir(root).arg("publish");

    if let Ok(registry) = std::env::var("SPUB_REGISTRY") {
        cmd.env("CARGO_REGISTRY_DEFAULT", &registry)
            .arg("--registry")
            .arg(registry)
            .arg("--token")
            .arg(std::env::var("SPUB_REGISTRY_TOKEN").unwrap());
    }

    if !verify {
        cmd.arg("--no-verify");
    }

    if !cmd
        .arg("--allow-dirty")
        .arg("--quiet")
        .arg("-p")
        .arg(krate)
        .status()?
        .success()
    {
        anyhow::bail!("Failed to publish crate {krate}");
    };

    Ok(())
}
