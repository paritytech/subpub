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
    let args = Args::parse();

    match args.command {
        Command::PrepareForPublish(opts) => prepare_for_publish(opts),
        Command::DoPublish(opts) => do_publish(opts),
    }
}

fn prepare_for_publish(opts: CommonOpts) {
    // Load crate details:
    let crate_details = Crates::load_crates_in_workspace(opts.path);

    println!("{crate_details:#?}");
}

fn do_publish(opts: CommonOpts) {

}