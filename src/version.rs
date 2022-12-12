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

use std::cmp::Ordering;

pub use semver::Version;

/// Bumps a version for the purpose of signifying a breaking change
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

#[test]
#[cfg(feature = "test-0")]
fn test_bump_for_breaking_change() {
    use semver::Prerelease;

    // Reference: https://semver.org

    // Patch version is bumped to minor
    assert_eq!(
        bump_for_breaking_change(Version::new(0, 0, 1)),
        Version::new(0, 1, 0)
    );

    // Minor version is bumped by its minor version component
    assert_eq!(
        bump_for_breaking_change(Version::new(0, 1, 0)),
        Version::new(0, 2, 0)
    );

    // Major version is bumped by its major version component
    assert_eq!(
        bump_for_breaking_change(Version::new(1, 0, 0)),
        Version::new(2, 0, 0)
    );

    // Major version is bumped by its major version component
    assert_eq!(
        bump_for_breaking_change({
            let mut version = Version::new(0, 0, 1);
            version.pre = Prerelease::new("dev").unwrap();
            version
        }),
        Version::new(0, 0, 1)
    );
}

pub fn maybe_bump_for_breaking_change(
    prev_versions: Vec<Version>,
    mut current_version: Version,
) -> Option<Version> {
    prev_versions
        .into_iter()
        .max()
        .map(|latest_version| {
            let max_version = match &current_version.cmp(&latest_version) {
                Ordering::Greater => current_version.to_owned(),
                _ => latest_version,
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

#[test]
#[cfg(feature = "test-0")]
fn test_maybe_bump_for_breaking_change() {
    use semver::Prerelease;

    // Picks the highest version among (previous versions + the current version)
    // when previous versions have the highest version
    assert_eq!(
        maybe_bump_for_breaking_change(vec![Version::new(0, 2, 0)], Version::new(0, 1, 0)),
        Some(Version::new(0, 3, 0))
    );

    // Picks the highest version among (previous versions + the current version)
    // when the current version is the highest version
    assert_eq!(
        maybe_bump_for_breaking_change(vec![Version::new(0, 1, 0)], Version::new(0, 2, 0)),
        Some(Version::new(0, 3, 0))
    );

    // Avoids producing a new version if there's no previous version and the
    // current version doesn't have a pre-release component
    assert_eq!(
        maybe_bump_for_breaking_change(vec![], Version::new(0, 1, 0)),
        None
    );

    // Produces a new version if there's no previous version and the current
    // version has a pre-release component
    assert_eq!(
        maybe_bump_for_breaking_change(vec![], {
            let mut version = Version::new(0, 0, 1);
            version.pre = Prerelease::new("dev").unwrap();
            version
        }),
        Some(Version::new(0, 0, 1))
    );
}
