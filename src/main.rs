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
mod logging;
mod version;

use anyhow::Context;
use clap::{Parser, Subcommand};
use crates::Crates;
use git::git_checkpoint_revert_all;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tracing::{info, span, Level};
use tracing_log::LogTracer;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Parser, Debug, Clone)]
struct CleanOpts {
    #[clap(long, help = "Path to the workspace root")]
    path: PathBuf,
}

#[derive(Parser, Debug, Clone)]
struct CheckOpts {
    #[clap(long, help = "Path to the workspace root")]
    path: PathBuf,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[clap(about = "Publish crates in order from least to most dependees")]
    Publish(PublishOpts),
    #[clap(about = "Revert all the commits made by subpub")]
    Clean(CleanOpts),
    #[clap(about = "Check that all crates are compliant to crates.io")]
    Check(CheckOpts),
}

#[derive(Parser, Debug, Clone)]
struct PublishOpts {
    #[clap(long, help = "Path to the workspace root")]
    path: PathBuf,

    #[clap(short = 'c', long = "crate", help = "Crates to be published")]
    crates: Vec<String>,

    #[clap(
        short = 's',
        long = "start-from",
        help = "Start publishing from this crate"
    )]
    start_from: Option<String>,

    #[clap(
        short = 'e',
        long = "exclude",
        help = "Crates to be excluded from the publishing process"
    )]
    exclude: Vec<String>,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stdout))
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(tracing_subscriber::filter::LevelFilter::ERROR),
        )
        .with(logging::CustomLayer)
        .init();

    let args = Args::parse();

    match args.command {
        Command::Publish(opts) => publish(opts),
        Command::Clean(opts) => git_checkpoint_revert_all(opts.path),
        Command::Check(opts) => check(opts),
    }
}

fn check(_opts: CheckOpts) -> anyhow::Result<()> {
    todo!("Implement check");
}

fn publish(opts: PublishOpts) -> anyhow::Result<()> {
    let mut version_bumps = HashMap::new();
    let mut crates = Crates::load_crates_in_workspace(opts.path.clone())?;

    struct OrderedCrate {
        name: String,
        rank: usize,
    }
    let mut publish_order: Vec<OrderedCrate> = vec![];
    loop {
        let mut progressed = false;
        for (krate, details) in &crates.details {
            if publish_order
                .iter()
                .any(|ord_crate| ord_crate.name == *krate)
            {
                continue;
            }
            let deps: HashSet<&String> = HashSet::from_iter(details.deps_relevant_during_publish());
            let ordered_deps = publish_order
                .iter()
                .filter(|ord_crate| deps.iter().any(|dep| **dep == ord_crate.name))
                .collect::<Vec<_>>();
            if ordered_deps.len() == deps.len() {
                publish_order.push(OrderedCrate {
                    rank: ordered_deps.iter().fold(1usize, |acc, ord_crate| {
                        acc.checked_add(ord_crate.rank).unwrap()
                    }),
                    name: krate.into(),
                });
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    publish_order.sort_by(|a, b| {
        use std::cmp::Ordering;
        match a.rank.cmp(&b.rank) {
            Ordering::Equal => a.name.cmp(&b.name),
            other => other,
        }
    });
    let publish_order: Vec<String> = publish_order
        .into_iter()
        .map(|ord_crate| ord_crate.name)
        .collect();
    info!(
        "Defined the overall publish order: {}\n",
        publish_order
            .iter()
            .map(|krate| krate.to_owned())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let unordered_crates = crates
        .details
        .keys()
        .filter(|krate| !publish_order.iter().any(|ord_crate| ord_crate == *krate))
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

    let input_crates = if !opts.crates.is_empty() {
        opts.crates.clone()
    } else {
        publish_order
            .clone()
            .into_iter()
            .filter(|krate| {
                if opts
                    .exclude
                    .iter()
                    .any(|excluded_crate| excluded_crate == krate)
                {
                    return false;
                }

                let details = crates
                    .details
                    .get(krate)
                    .with_context(|| format!("Crate not found: {krate}"))
                    .unwrap();
                details.should_be_published
            })
            .collect()
    };
    let (selected_crates, selected_crates_order) = if let Some(start_from) = opts.start_from {
        let mut keep = false;
        let selected_crates = input_crates
            .into_iter()
            .filter(|krate| {
                if *krate == start_from {
                    keep = true;
                }
                keep
            })
            .collect::<Vec<_>>();

        let mut keep = false;
        let selected_crates_order = publish_order
            .iter()
            .filter(|krate| {
                if **krate == start_from {
                    keep = true;
                }
                keep && selected_crates.iter().any(|sel_crate| sel_crate == *krate)
            })
            .collect::<Vec<_>>();

        (selected_crates, selected_crates_order)
    } else {
        let selected_crates_order = publish_order
            .iter()
            .filter(|ord_crate| {
                input_crates
                    .iter()
                    .any(|sel_crate| *sel_crate == **ord_crate)
            })
            .collect::<Vec<_>>();

        (input_crates, selected_crates_order)
    };

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

    fn check_excluded_crates(
        crates: &Crates,
        initial_crate: &String,
        parent_crate: Option<&String>,
        krate: &String,
        excluded_crates: &Vec<String>,
        visited_crates: &[&String],
    ) -> anyhow::Result<()> {
        if visited_crates
            .iter()
            .any(|visited_crate| *visited_crate == krate)
        {
            return Ok(());
        }

        if parent_crate.is_some() {
            if excluded_crates
                .iter()
                .any(|excluded_crate| excluded_crate == krate)
            {
                if let Some(parent_crate) = parent_crate {
                    anyhow::bail!("Crate {krate} was excluded from CLI options, but it is a dependency of {parent_crate}, and that is a dependency of {initial_crate}, which would be published.");
                } else {
                    anyhow::bail!("Crate {krate} was excluded from CLI options, but it is a dependency of {initial_crate}, which would be published.");
                }
            }
        } else if excluded_crates
            .iter()
            .any(|excluded_crate| excluded_crate == krate)
        {
            return Ok(());
        }

        let details = crates
            .details
            .get(krate)
            .with_context(|| format!("Crate not found: {krate}"))?;
        if !details.should_be_published {
            if let Some(parent_crate) = parent_crate {
                anyhow::bail!("Crate {krate} should not be published, but it is a dependency of {parent_crate}, and that is a dependency of {initial_crate}, which would be published. Check if {krate} has \"publish = false\" in {:?}.", details.toml_path);
            } else {
                anyhow::bail!("Crate {krate} should not be published, but it is a dependency of {initial_crate}, which would be published. Check if {krate} has \"publish = false\" in {:?}.", details.toml_path);
            }
        }

        for dep in &details.deps {
            let visited_crates = visited_crates
                .iter()
                .copied()
                .chain(vec![krate].into_iter())
                .collect::<Vec<_>>();
            check_excluded_crates(
                crates,
                initial_crate,
                if krate == initial_crate {
                    None
                } else {
                    Some(krate)
                },
                dep,
                excluded_crates,
                &visited_crates,
            )?;
        }

        Ok(())
    }
    for krate in &selected_crates {
        check_excluded_crates(&crates, krate, None, krate, &opts.exclude, &[])?;
    }

    info!(
        "Processing crates in this order: {}\n",
        selected_crates_order
            .iter()
            .map(|krate| (*krate).into())
            .collect::<Vec<String>>()
            .join(", ")
    );

    let mut processed_crates: HashSet<String> = HashSet::new();
    for sel_crate in selected_crates_order {
        let span = span!(Level::INFO, "order", crate = sel_crate);
        let _ = span.enter();

        if processed_crates.get(sel_crate).is_some() {
            info!("[{sel_crate}] Crate was already processed",);
            continue;
        }
        processed_crates.insert(sel_crate.into());

        info!("[{sel_crate}] Processing crate");

        let details = crates.details.get(sel_crate).unwrap();

        for krate in &publish_order {
            if krate == sel_crate {
                break;
            }
            let crate_details = crates
                .details
                .get(krate)
                .with_context(|| format!("Crate details not found for crate: {krate}"))?;
            details.write_dependency_version(krate, &crate_details.version)?;
        }

        let crates_to_publish = crates.what_needs_publishing(sel_crate, &publish_order)?;

        if crates_to_publish.is_empty() {
            info!("[{sel_crate}] Crate does not need to be published");
            continue;
        } else if crates_to_publish.len() == 1 {
            info!("[{sel_crate}] Publishing crate {}", crates_to_publish[0])
        } else {
            info!(
              "[{sel_crate}] Crates will be processed in the following order for publishing {sel_crate}: {}",
              crates_to_publish
                  .iter()
                  .map(|krate| (krate).into())
                  .collect::<Vec<String>>()
                  .join(", ")
          );
        }

        for krate in crates_to_publish {
            if processed_crates.get(sel_crate).is_some() {
                info!("[{sel_crate}] Crate {krate} was already processed",);
                continue;
            }

            let details = crates.details.get(&krate).unwrap();

            if details.needs_publishing(&opts.path)? {
                crates.maybe_bump_crate_version(&krate, &opts.path, &mut version_bumps)?;
                crates.strip_dev_deps_and_publish(&krate)?;
            } else {
                info!("[{sel_crate}] Crate {krate} does not need to be published");
            }

            processed_crates.insert(krate);
        }
    }

    // git_checkpoint_revert_all(&opts.path)?;
    // for krate in &published_crates {
    //     let details = crates.details.get(krate).unwrap();
    //     for other_crate in &publish_order {
    //         let other_crate_details = crates
    //             .details
    //             .get(other_crate)
    //             .with_context(|| format!("Crate not found: {}", other_crate))?;
    //         other_crate_details.write_dependency_version(krate, &details.version)?;
    //     }
    // }
    //
    // let mut cmd = std::process::Command::new("cargo");
    // let mut cmd = cmd.current_dir(&opts.path).arg("update");
    // for krate in &published_crates {
    //     cmd = cmd.arg("-p").arg(krate);
    // }
    // if !cmd.status()?.success() {
    //     anyhow::bail!("Command failed: {cmd:?}");
    // };

    Ok(())
}
