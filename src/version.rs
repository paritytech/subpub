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

pub use semver::Version;
use std::cmp::Ordering;

fn bump_for_breaking_change(mut version: Version) -> Version {
    if version.pre != semver::Prerelease::EMPTY {
        version.pre = semver::Prerelease::EMPTY;
    } else if version.major == 0 {
        version.minor += 1;
        version.patch = 0;
    } else {
        version.major += 1;
        version.minor = 0;
        version.patch = 0;
    }
    version
}

/// Bump the version for a breaking change and to release. Examples of bumps carried out:
///
/// ```text
/// 0.15.0 -> 0.16.0 (bump minor if 0.x.x)
/// 4.0.0 -> 5.0.0 (bump major if >1.0.0)
/// 4.0.0-dev -> 4.0.0 (remove prerelease label)
/// 4.0.0+buildmetadata -> 5.0.0+buildmetadata (preserve build metadata regardless)
/// ```
///
/// Return the new version.
pub fn maybe_bump_for_breaking_change(
    prev_versions: Vec<Version>,
    mut current_version: Version,
) -> Option<Version> {
    println!("prev_versions {:?}", prev_versions);
    prev_versions
        .into_iter()
        .max()
        .map(|max_prev_version| {
            let max_version = match &current_version.cmp(&max_prev_version) {
                Ordering::Greater => (&current_version).to_owned(),
                _ => max_prev_version,
            };
            bump_for_breaking_change(max_version)
        })
        .or_else(|| {
            if current_version.pre == semver::Prerelease::EMPTY {
                None
            } else {
                current_version.pre = semver::Prerelease::EMPTY;
                Some(current_version)
            }
        })
}
