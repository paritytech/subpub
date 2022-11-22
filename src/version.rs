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
pub fn bump_for_breaking_change(
    prev_versions: Vec<Version>,
    mut current_version: Version,
) -> Option<Version> {
    prev_versions
        .into_iter()
        .max()
        .map(
            |mut max_prev_version| match &current_version.cmp(&max_prev_version) {
                Ordering::Greater => {
                    if max_prev_version.major != 0 && current_version.major == 0 {
                        let mut current_version = (&current_version).to_owned();
                        current_version.major = max_prev_version.major + 1;
                        current_version.minor = 0;
                        current_version.patch = 0;
                        current_version.pre = semver::Prerelease::EMPTY;
                        Some(current_version)
                    } else {
                        None
                    }
                }
                _ => {
                    max_prev_version.pre = semver::Prerelease::EMPTY;
                    if max_prev_version.major == 0 {
                        max_prev_version.minor += 1;
                        max_prev_version.patch = 0;
                    } else {
                        max_prev_version.major += 1;
                        max_prev_version.minor = 0;
                        max_prev_version.patch = 0;
                    }
                    Some(max_prev_version)
                }
            },
        )
        .flatten()
        .or_else(|| {
            if current_version.pre == semver::Prerelease::EMPTY {
                None
            } else {
                current_version.pre = semver::Prerelease::EMPTY;
                Some(current_version)
            }
        })
}
