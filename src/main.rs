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

fn main() {
    env_logger::init();

    let args = Args::parse();

    let res = match args.command {
        Command::Publish(opts) => publish(opts),
        Command::Clean(opts) => git_checkpoint_revert_all(opts.path),
        Command::Check(opts) => check(opts),
    };

    if let Err(e) = res {
        log::error!("{e:?}");
    }
}

fn check(_opts: CheckOpts) -> anyhow::Result<()> {
    todo!("Implement check");
}

fn publish(opts: PublishOpts) -> anyhow::Result<()> {
    let mut needs_publishing = HashMap::new();
    let mut crates = Crates::load_crates_in_workspace(opts.path.clone())?;

    let mut publish_order: Vec<(usize, String)> = vec![];
    loop {
        let mut progressed = false;
        for (krate, details) in &crates.details {
            if publish_order
                .iter()
                .any(|(_, ord_crate)| ord_crate == krate)
            {
                continue;
            }
            // Does not include dev_dependencies because they are not relevant for publishing purposes
            let mut deps: HashSet<&String> = HashSet::from_iter(details.deps.iter());
            for dep in details.build_deps.iter() {
                deps.insert(dep);
            }
            let ordered_deps = publish_order
                .iter()
                .filter(|(_, ord_crate)| deps.iter().any(|dep| *dep == ord_crate))
                .collect::<Vec<_>>();
            if ordered_deps.len() == deps.len() {
                publish_order.push((
                    ordered_deps
                        .iter()
                        .fold(1, |acc, (rank, _)| acc.checked_add(*rank).unwrap()),
                    krate.into(),
                ));
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    publish_order.sort_by(|a, b| {
        if a.0 == b.0 {
            a.1.cmp(&b.1)
        } else {
            a.0.cmp(&b.0)
        }
    });
    let publish_order: Vec<String> = publish_order
        .into_iter()
        .map(|(_, ord_crate)| ord_crate)
        .collect();
    println!(
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

    let input_crates = if opts.crates.len() > 0 {
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
        visited_crates: &Vec<&String>,
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
                .map(|visited_crate| *visited_crate)
                .chain(vec![].into_iter())
                .collect();
            check_excluded_crates(
                &crates,
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
        check_excluded_crates(&crates, krate, None, krate, &opts.exclude, &vec![])?;
    }

    println!(
        "Processing crates in this order: {}\n",
        selected_crates_order
            .iter()
            .map(|krate| (*krate).into())
            .collect::<Vec<String>>()
            .join(", ")
    );

    let mut published_crates: HashSet<String> = HashSet::new();
    for sel_crate in selected_crates_order {
        if published_crates.get(sel_crate).is_some() {
            println!("[{sel_crate}] Crate has already been published");
            continue;
        }
        published_crates.insert(sel_crate.into());
        println!("[{sel_crate}] Processing crate");

        let details = crates.details.get(sel_crate).unwrap();

        for krate in &publish_order {
            if krate == sel_crate {
                break;
            }
            if krate == "substrate-prometheus-endpoint" {
                println!("HIT! {sel_crate}")
            }
            let crate_details = crates
                .details
                .get(krate)
                .with_context(|| format!("Crate details not found for crate: {krate}"))?;
            details.write_dependency_version(krate, &crate_details.version)?;
        }

        let crates_to_publish = crates.what_needs_publishing(
            vec![sel_crate.into()],
            &opts.path,
            &mut needs_publishing,
        )?;

        if crates_to_publish.is_empty() {
            println!("[{sel_crate}] Crate does not need to be published");
            continue;
        } else if crates_to_publish.len() == 1 {
            println!("[{sel_crate}] Publishing crate {}", crates_to_publish[0])
        } else {
            println!(
              "[{sel_crate}] Crates will be published in the following order for publishing {sel_crate}: {}",
              crates_to_publish
                  .iter()
                  .map(|krate| krate.into())
                  .collect::<Vec<String>>()
                  .join(", ")
          );
        }

        for krate in crates_to_publish {
            if crates.does_crate_version_need_bumping_to_publish(
                &krate,
                &opts.path,
                &mut needs_publishing,
            )? {
                let (old_version, new_version) =
                    crates.bump_crate_version_for_breaking_change(&krate)?;
                println!("[{sel_crate}] Bumped crate {krate} from {new_version} to {old_version}");
            }

            crates.strip_dev_deps_and_publish(&krate)?;
            needs_publishing.insert((&krate).into(), false);

            let published_crate_details = crates
                .details
                .get(&krate)
                .with_context(|| format!("Crate not found: {krate}"))?;
            for other_crate in &publish_order {
                let other_crate_details = crates
                    .details
                    .get(other_crate)
                    .with_context(|| format!("Crate not found: {}", other_crate))?;
                other_crate_details
                    .write_dependency_version(&krate, &published_crate_details.version)?;
            }

            published_crates.insert(krate);
        }
    }

    git_checkpoint_revert_all(&opts.path)?;
    for krate in &published_crates {
        let details = crates.details.get(krate).unwrap();
        for other_crate in &publish_order {
            let other_crate_details = crates
                .details
                .get(other_crate)
                .with_context(|| format!("Crate not found: {}", other_crate))?;
            other_crate_details.write_dependency_version(krate, &details.version)?;
        }
    }

    let mut cmd = std::process::Command::new("cargo");
    let mut cmd = cmd.current_dir(&opts.path).arg("update");
    for krate in &published_crates {
        cmd = cmd.arg("-p").arg(krate);
    }
    if !cmd.status()?.success() {
        anyhow::bail!("Command failed: {cmd:?}");
    };

    Ok(())
}
