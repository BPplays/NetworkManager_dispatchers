#[allow(unused_imports)]
use tracing::{
    trace,
    debug,
    info,
    warn,
    error,
};

use clap;
use common::DispatcherArgs;

#[derive(Debug, clap::Parser)]
pub struct MyDispatcherArgs {
    #[command(flatten)]
    pub dispatcher: DispatcherArgs,
}

fn main() {
    println!("Hello, world!");
}
