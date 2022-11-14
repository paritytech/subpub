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

mod crate_details;
mod crates;
mod external;
mod version;

use clap::{Parser, Subcommand};
use crates::Crates;
use std::path::PathBuf;

/// Release crates and their dependencies from a workspace
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(subcommand)]
    command: Command,
}

// Separate help text to preserve newlines.
const DO_PUBLISH_HELP: &str = "\
Given some crates you'd like to publish, this will:
  - Find everything that needs publishing to support this, and
    complain if anything needs a version bump to be published (run
    prepare-for-publish first).
  - Publish each crate in the correct order, stripping dev
    dependencies and waiting as needed between publishes.
";

// Separate help text to preserve newlines.
const PREPARE_FOR_PUBLISH_HELP: &str = "\
Given some crates you'd like to publish, this will:
  - Find everything that needs publishing to support this (ie
    all dependencies that have also changed since they were last
    published.
  - Bump any versions of crates that need publishing (this assumes
    that we always do breaking change bumps)
  - Update the lockfile to accomodate the above.
";

#[derive(Subcommand, Debug)]
enum Command {
    #[clap(long_about = PREPARE_FOR_PUBLISH_HELP)]
    PrepareForPublish(CommonOpts),
    #[clap(long_about = DO_PUBLISH_HELP)]
    DoPublish(CommonOpts),
    #[clap(about = "Publish crates in order from least to most dependees")]
    PublishInOrder(CommonOpts),
}

#[derive(Parser, Debug, Clone)]
struct CommonOpts {
    /// Path to the workspace root.
    #[clap(long, default_value = ".")]
    path: PathBuf,

    /// Crates you'd like to publish.
    #[clap(short = 'c', long = "crate")]
    crates: Vec<String>,
}

fn main() {
    env_logger::init();

    let args = Args::parse();

    let res = match args.command {
        Command::PrepareForPublish(opts) => prepare_for_publish(opts, false),
        Command::DoPublish(opts) => do_publish(opts),
        Command::PublishInOrder(opts) => publish_in_order(opts),
    };

    if let Err(e) = res {
        log::error!("{e:?}");
    }
}

fn prepare_for_publish(opts: CommonOpts, publish: bool) -> anyhow::Result<()> {
    // Run the logic first, and then print the various details, so that
    // our logging is all nicely separated from our output.
    let mut crates = Crates::load_crates_in_workspace(opts.path)?;
    let publish_these = crates.what_needs_publishing(opts.crates.clone())?;

    let mut no_need_to_bump = vec![];
    let mut bump_these = vec![];
    for name in &publish_these {
        if crates.does_crate_version_need_bumping_to_publish(&name)? {
            let (old_version, new_version) =
                crates.bump_crate_version_for_breaking_change(&name)?;
            bump_these.push((name, old_version, new_version));
        } else {
            no_need_to_bump.push(name);
        }
    }

    crates.update_lockfile_for_crates(opts.crates.clone())?;

    println!("\nYou've said you'd like to publish these crates:\n");
    for name in &opts.crates {
        println!("  {name}");
    }

    println!("\nThe following crates need publishing (in this order) in order to do this:\n");
    for name in &publish_these {
        println!("  {name}");
    }

    if !bump_these.is_empty() {
        println!("\nI'm bumping the following crate versions to accomodate this:\n");
        for (name, old_version, new_version) in bump_these {
            println!("  {name}: {old_version} -> {new_version}");
        }
    } else {
        println!("\nNo crates needed a version bump to accomodate this\n");
    }

    if !no_need_to_bump.is_empty() {
        println!("\nThese crates did not need a version bump in order to publish:\n");
        for name in no_need_to_bump {
            println!("  {name}");
        }
    }

    if publish {
        for name in publish_these {
            crates.strip_dev_deps_and_publish(&name)?;
        }
    } else {
        println!("\nNow, you can create a release PR to have these version bumps merged");
    }

    Ok(())
}

fn do_publish(opts: CommonOpts) -> anyhow::Result<()> {
    // Run the logic first, and then print the various details, so that
    // our logging is all nicely separated from our output.
    let crates = Crates::load_crates_in_workspace(opts.path)?;
    let publish_these = crates.what_needs_publishing(opts.crates.clone())?;

    // Check that no versions need bumping.
    let mut bump_these = vec![];
    for name in &publish_these {
        if crates.does_crate_version_need_bumping_to_publish(&name)? {
            bump_these.push(&**name);
        }
    }

    if !bump_these.is_empty() {
        anyhow::bail!(
            "The following crates need a version bump before they can be published: {}",
            bump_these.join(", ")
        );
    }

    println!("\nYou've said you'd like to publish these crates:\n");
    for name in &opts.crates {
        println!("  {name}");
    }

    println!("\nThe following crates need publishing (in this order) in order to do this:\n");
    for name in &publish_these {
        println!("  {name}");
    }

    println!("\nNote: This will strip dev dependencies from crates being published! Remember to revert those changes after publishing.");

    for name in publish_these {
        crates.strip_dev_deps_and_publish(&name)?;
    }
    Ok(())
}

fn publish_in_order(opts: CommonOpts) -> anyhow::Result<()> {
    let crates = Crates::load_crates_in_workspace(opts.path.clone())?;

    let selected_crates = if opts.crates.len() > 0 {
        opts.crates.clone()
    } else {
        crates
            .details
            .keys()
            .map(|crate_name| crate_name.into())
            .collect()
    };

    let mut order: Vec<&String> = vec![];
    loop {
        let mut progressed = false;
        for (crate_name, details) in crates.details.iter() {
            if order
                .iter()
                .any(|ord_crate_name| *ord_crate_name == crate_name)
            {
                continue;
            }
            if details.deps.is_empty()
                || details.deps.iter().all(|dep_crate_name| {
                    order
                        .iter()
                        .any(|ord_crate_name| *ord_crate_name == dep_crate_name)
                })
            {
                order.push(crate_name);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }

    let unordered_crates = crates
        .details
        .keys()
        .filter(|crate_name| {
            !order
                .iter()
                .any(|ord_crate_name| ord_crate_name == crate_name)
        })
        .collect::<Vec<_>>();
    if !unordered_crates.is_empty() {
        anyhow::bail!(
            "Unable to determine publish order for the following crates: {}",
            unordered_crates
                .iter()
                .map(|crate_name| (*crate_name).into())
                .collect::<Vec<String>>()
                .join(", ")
        );
    }

    let selected_crates_order = order
        .iter()
        .filter(|crate_name| {
            selected_crates
                .iter()
                .any(|sel_crate| sel_crate == **crate_name)
        })
        .collect::<Vec<_>>();

    println!(
        "Publishing crates in this order: {}",
        selected_crates_order
            .iter()
            .map(|crate_name| (**crate_name).into())
            .collect::<Vec<String>>()
            .join(", ")
    );

    for crate_name in &selected_crates_order {
        let opts = opts.clone();
        prepare_for_publish(
            CommonOpts {
                crates: vec![(**crate_name).into()],
                ..opts
            },
            true,
        )?;
        break;
    }

    Ok(())
}
