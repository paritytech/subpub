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
mod git;
mod version;

use crate::git::*;
use anyhow::Context;
use clap::{Parser, Subcommand};
use crates::Crates;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Release crates and their dependencies from a workspace
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Parser, Debug, Clone)]
struct CleanOpts {
    /// Path to the workspace root.
    #[clap(long)]
    path: PathBuf,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[clap(about = "Publish crates in order from least to most dependees")]
    PublishInOrder(CommonOpts),
    Clean(CleanOpts),
}

#[derive(Parser, Debug, Clone)]
struct CommonOpts {
    /// Path to the workspace root.
    #[clap(long, default_value = ".")]
    path: PathBuf,

    /// Crates you'd like to publish.
    #[clap(short = 'c', long = "crate")]
    crates: Vec<String>,

    /// Crates you'd like to publish.
    #[clap(short = 's', long = "start-from")]
    start_from: Option<String>,
}

fn main() {
    env_logger::init();

    let args = Args::parse();

    let res = match args.command {
        Command::PublishInOrder(opts) => publish_in_order(opts),
        Command::Clean(opts) => git_checkpoint_revert_all(opts.path),
    };

    if let Err(e) = res {
        log::error!("{e:?}");
    }
}

fn publish_in_order(opts: CommonOpts) -> anyhow::Result<()> {
    let mut cio = HashMap::new();
    let mut crates = Crates::load_crates_in_workspace(opts.path.clone())?;

    let selected_crates = if opts.crates.len() > 0 {
        opts.crates.clone()
    } else {
        crates.details.keys().map(|krate| krate.into()).collect()
    };

    let mut all_orders: Vec<Vec<String>> = vec![];
    loop {
        let mut order = vec![];
        let mut progressed = false;
        for (krate, details) in &crates.details {
            if order.iter().any(|ord_crate| ord_crate == krate) {
                continue;
            }
            let all_deps: Vec<_> = details
                .deps
                .iter()
                .chain(details.build_deps.iter())
                .collect();
            if all_deps.is_empty()
                || all_deps.iter().all(|dep_crate| {
                    order.iter().any(|ord_crate| ord_crate == *dep_crate)
                        || all_orders
                            .iter()
                            .any(|order| order.iter().any(|ord_crate| ord_crate == *dep_crate))
                })
            {
                order.push(krate.into());
                progressed = true;
            }
        }
        if progressed {
            all_orders.push(order);
        } else {
            break;
        }
    }
    let order: Vec<String> = all_orders
        .into_iter()
        .map(|mut order| {
            order.sort();
            order
        })
        .flatten()
        .collect();

    let unordered_crates = crates
        .details
        .keys()
        .filter(|krate| !order.iter().any(|ord_crate| ord_crate == *krate))
        .collect::<Vec<_>>();
    if !unordered_crates.is_empty() {
        anyhow::bail!(
            "Failed to determine publish order for the following crates: {}",
            unordered_crates
                .iter()
                .map(|krate| (*krate).into())
                .collect::<Vec<String>>()
                .join(", ")
        );
    }

    let selected_crates_order = order
        .iter()
        .filter(|krate| {
            selected_crates
                .iter()
                .any(|sel_crate| *sel_crate == **krate)
        })
        .collect::<Vec<_>>();

    let unordered_selected_crates = selected_crates
        .iter()
        .filter(|sel_crate| {
            !selected_crates_order
                .iter()
                .any(|sel_crate_ordered| sel_crate_ordered == sel_crate)
        })
        .collect::<Vec<_>>();
    if !unordered_selected_crates.is_empty() {
        anyhow::bail!(
            "Failed to determine publish order for the following selected crates: {}",
            unordered_selected_crates
                .iter()
                .map(|krate| (*krate).into())
                .collect::<Vec<String>>()
                .join(", ")
        );
    }

    println!(
        "Processing crates in this order: {}",
        selected_crates_order
            .iter()
            .map(|krate| (*krate).into())
            .collect::<Vec<String>>()
            .join(", ")
    );

    let mut dealt_with_crates: HashSet<String> = HashSet::new();
    for selected_crate in selected_crates_order {
        if dealt_with_crates.get(selected_crate).is_some() {
            println!("[{selected_crate}] Crate has already been dealt with");
            continue;
        }
        dealt_with_crates.insert(selected_crate.into());

        let details = crates.details.get(selected_crate).unwrap();

        for prev_crate in &order {
            if prev_crate != selected_crate {
                let prev_crate_details = crates
                    .details
                    .get(prev_crate)
                    .with_context(|| format!("Crate not found: {prev_crate}"))?;
                details.write_dependency_version(prev_crate, &prev_crate_details.version)?;
            }
        }

        let crates_set_to_publish =
            crates.what_needs_publishing(vec![selected_crate.into()], &mut cio)?;
        let crates_to_publish = order
            .iter()
            .filter(|ordered_crate| {
                crates_set_to_publish
                    .iter()
                    .any(|crate_set_to_publish| crate_set_to_publish == *ordered_crate)
                    && !dealt_with_crates
                        .iter()
                        .any(|dealt_with_crate| dealt_with_crate == *ordered_crate)
            })
            .map(|krate| krate.into())
            .collect::<Vec<String>>();

        if crates_to_publish.is_empty() {
            println!("[{selected_crate}] Crate and its dependencies do not need to be published");
            continue;
        } else if crates_to_publish.len() > 1 {
            println!(
              "[{selected_crate}] Crates will be published in the following order for publishing {selected_crate}: {}",
              crates_to_publish
                  .iter()
                  .map(|krate| krate.into())
                  .collect::<Vec<String>>()
                  .join(", ")
          );
        } else {
            println!(
                "[{selected_crate}] Publishing crate {}",
                crates_to_publish[0]
            )
        }

        for krate in crates_to_publish {
            while crates.does_crate_version_need_bumping_to_publish(&krate, &mut cio)? {
                let (old_version, new_version) =
                    crates.bump_crate_version_for_breaking_change(&krate)?;
                println!(
                    "[{selected_crate}] Bumping crate {krate} from {new_version} to {old_version}"
                );
            }

            crates.strip_dev_deps_and_publish(&krate, &mut cio)?;

            let published_crate_details = crates
                .details
                .get(&krate)
                .with_context(|| format!("Crate not found: {krate}"))?;
            for next_crate in &order {
                let next_crate_details = crates
                    .details
                    .get(next_crate)
                    .with_context(|| format!("Crate not found: {next_crate}"))?;
                next_crate_details
                    .write_dependency_version(&krate, &published_crate_details.version)?;
            }

            crates.update_lockfile_for_crates(vec![&krate])?;

            dealt_with_crates.insert(krate);
        }
    }

    Ok(())
}
