use std::{net::IpAddr, str::FromStr, time::Duration};

use bytesize::ByteSize;

#[allow(unused_imports)]
use tracing::{
    trace,
    debug,
    info,
    warn,
    error,
};

use clap;
use clap::Parser;
use common::{
    DispatcherArgs,
    LogArgs,
    init_logging,
    set_niceness,
    Nice,
};

use tzupdate::timezone_lookup as tzget;



#[derive(Debug, clap::Parser)]
pub struct Args {
    #[command(flatten)]
    pub dispatcher: DispatcherArgs,

    #[command(flatten)]
    pub log: LogArgs,

}

#[tokio::main]
async fn main() {
	let args = Args::parse();

    match set_niceness(Nice::try_from(19).unwrap()) {
        Ok(_) => {}
        Err(e) => {
            eprintln!("{}", e)
        }
    }

	let _guard = init_logging(
		args.log.file.as_deref(),
		args.log.level,
		args.log.stdout_timestamps,
        ByteSize::mb(5),
        3,
	).unwrap();

    match tzget::get_timezone_consensus(
        None,
        Duration::from_secs(5),
    ) {
        Ok(timezone) => info!("{}", timezone),
        Err(e) => error!("Failed to determine timezone: {}", e),
    }
}
