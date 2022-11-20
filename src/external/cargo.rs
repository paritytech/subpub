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

/// Update the lockfile for dependencies given and any of their subdependencies.
pub fn publish_crate(root: &Path, package: &str) -> anyhow::Result<()> {
    let mut cmd = Command::new("cargo");

    if let Ok(registry) = std::env::var("SUBPUB_REGISTRY") {
        cmd.env("CARGO_REGISTRIES_{}_INDEX", registry.to_uppercase())
            .arg("--registry")
            .arg(registry)
            .arg("--token")
            .arg(std::env::var("SUBPUB_CARGO_TOKEN").unwrap());
    }

    if !cmd
        .current_dir(&root)
        .arg("publish")
        .arg("--locked")
        .arg("--allow-dirty")
        .arg("-vv")
        .arg("-p")
        .arg(package)
        .status()?
        .success()
    {
        anyhow::bail!("Failed to publish crate {package}");
    };

    Ok(())
}
