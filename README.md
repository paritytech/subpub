# SubPub

A tool to help you publish crates from Substrate. Currently very early in development and should still be considered alpha quality.

Roughly, this tool takes inspiration from `cargo-unleash`, and is focused on automating as far as possible the workflow for publishing a subset of the crates that we need from substrate.

The motivation for creating this tool is to assist in publishing a subset of the Substrate crates that we need for Subxt.

Roughly, this tool can take care of the following steps:
- For a given crate or crates you'd like to publish, find all of the dependencies we may also need to publish.
- Compare local source against versions published on crates.io to find out whether a crate needs a version bump.
- Perform the version bumping.
- Publish this set of crates in the correct order to crates.io.

Run the crate with `cargo run` to see the available commands and help text for them.

# Contributions

Contributions are welcome, but please open an issue first to discuss.

While I'd like this tool to be fairly Substrate agnostic and work in any cargo workspace, its focus is on helping publish Substrate crates, and so feature requests that complicate that to support other workflows/approaches may be rejected.