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
mod version;
mod crates_io;
mod cargo;

use clap::{ Parser, Subcommand };
use std::path::PathBuf;
use crates::Crates;

/// Release crates and their dependencies from a workspace
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Given some crates you'd like to publish, this will bump all
    /// versions as needed, and list all of the crates that actually need
    /// publishing (which may include dependencies and dependencies of
    /// dependencies and so on).
    PrepareForPublish(CommonOpts),
    /// Make sure that all the crates needed to be published can be
    /// (ie their versions are bumped accordingly), and publish them all in
    /// the correct order.
    DoPublish(CommonOpts)
}

#[derive(Parser, Debug)]
struct CommonOpts {
    /// Path to the workspace root.
    #[clap(long, default_value = ".")]
    path: PathBuf,

    /// Crates you'd like to publish.
    #[clap(long)]
    crates: Vec<String>,
}

fn main() {
    env_logger::init();

    let args = Args::parse();

    let res = match args.command {
        Command::PrepareForPublish(opts) => prepare_for_publish(opts),
        Command::DoPublish(opts) => do_publish(opts),
    };

    if let Err(e) = res {
        log::error!("{e:?}");
    }
}

fn prepare_for_publish(opts: CommonOpts) -> anyhow::Result<()> {
    // Run the logic first, and then print the various details, so that
    // our logging is all nicely separated from our output.
    let mut crate_details = Crates::load_crates_in_workspace(opts.path)?;
    let publish_these = crate_details.what_needs_publishing(opts.crates.clone())?;
    let mut no_need_to_bump = vec![];
    let mut bump_these = vec![];
    for name in &publish_these {
        if crate_details.does_crate_version_need_bumping_to_publish(&name)? {
            let (old_version, new_version) = crate_details.bump_crate_version_for_breaking_change(&name)?;
            bump_these.push((name, old_version, new_version));
        } else {
            no_need_to_bump.push(name);
        }
    }
    crate_details.update_lockfile_for_crates(opts.crates.clone())?;

    println!("You've said you'd like to publish these crates:\n");
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

    println!("\nNow, you can create a release PR to have these version bumps merged");
    Ok(())
}

fn do_publish(opts: CommonOpts) -> anyhow::Result<()>  {
    Ok(())
}