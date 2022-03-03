pub use semver::Version;

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
pub fn bump_for_breaking_change(version: Version) -> Version {
    let mut new_version = version.clone();

    if new_version.pre != semver::Prerelease::EMPTY {
        // Remove pre-release tag like `-dev` if present
        new_version.pre = semver::Prerelease::EMPTY;
    } else if new_version.major == 0 {
        // Else, bump minor if 0.x.0 crate
        new_version.minor += 1;
    } else {
        // Else bump major version
        new_version.major += 1;
    }

    new_version
}