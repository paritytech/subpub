mod one_crate;
mod crates;

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
        log::error!("{e}");
    }
}

fn prepare_for_publish(opts: CommonOpts) -> anyhow::Result<()> {
    let crate_details = Crates::load_crates_in_workspace(opts.path)?;
    println!("You've said you'd like to publish these crates:\n");
    for name in &opts.crates {
        println!("  {name}");
    }

    let publish_these = crate_details.what_needs_publishing(opts.crates)?;
    println!("\nThe following crates need publishing (in this order) in order to do this:\n");
    for name in &publish_these {
        println!("  {name}");
    }

    println!("\nI'm bumping the following crate versions to accomodate this:\n");
    let mut updated_details = crate_details.clone();
    for name in &publish_these {
        let (old_version, new_version) = updated_details.bump_crate_version(&name)?;
        println!("  {name}: {old_version} -> {new_version}");
    }

    println!("\nNow, you can create a release PR to have these version bumps merged");
    Ok(())
}

fn do_publish(opts: CommonOpts) -> anyhow::Result<()>  {
    Ok(())
}