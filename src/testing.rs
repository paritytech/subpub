#[allow(clippy::enum_variant_names)]
#[allow(dead_code)]
pub enum TestEnvironment {
    CrateNotPublishedIfUnchanged,
    CratePublishedIfNotPublished,
    CratePublishedIfChanged,
}
