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
    mut version: Version,
) -> Option<Version> {
    match prev_versions.into_iter().max() {
        Some(mut max_version) => match version.cmp(&max_version) {
            Ordering::Greater => None,
            _ => {
                max_version.pre = semver::Prerelease::EMPTY;
                if max_version.major == 0 {
                    max_version.minor += 1;
                    max_version.patch = 0;
                } else {
                    max_version.major += 1;
                    max_version.minor = 0;
                    max_version.patch = 0;
                }
                Some(max_version)
            }
        },
        None => {
            if version.pre == semver::Prerelease::EMPTY {
                None
            } else {
                version.pre == semver::Prerelease::EMPTY;
                Some(version)
            }
        }
    }
}
