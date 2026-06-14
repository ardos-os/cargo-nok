use std::{env, error::Error};

mod cargo;
mod cli;
mod kernel;
mod module;
mod output;
mod pipeline;
mod process;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn main() {
    if let Err(error) = run() {
        output::error(error);
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    if args.next().as_deref() != Some("nok") {
        return Err("cargo-nok must be invoked as `cargo nok`".into());
    }

    let Some(subcommand) = args.next() else {
        return Err("expected `build`, `expand`, `load`, `unload`, or `reload`".into());
    };

    match subcommand.as_str() {
        "build" => pipeline::build(args.collect()),
        "expand" => pipeline::expand(args.collect()),
        "load" => pipeline::load(args.collect()),
        "unload" => pipeline::unload(args.collect()),
        "reload" => pipeline::reload(args.collect()),
        _ => Err(format!("unsupported subcommand `{subcommand}`").into()),
    }
}
