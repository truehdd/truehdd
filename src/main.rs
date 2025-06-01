#![allow(dead_code)]

use anyhow::Result;
use clap::Parser as ClapParser;
use indicatif::MultiProgress;
use indicatif_log_bridge::LogWrapper;

use cli::command::{Cli, Commands, LogFormat};
use cli::decode::cmd_decode;
use cli::info::cmd_info;

mod byteorder;
mod caf;
mod cli;
mod damf;
mod input;
pub(crate) mod timestamp;

fn main() -> Result<()> {
    let cli = Cli::parse();

    let base_level = cli.loglevel.to_level_filter();

    let multi = MultiProgress::new();

    let mut env_builder = env_logger::Builder::from_default_env();
    env_builder.filter_level(base_level);
    match cli.log_format {
        LogFormat::Plain => {
            env_builder.format_timestamp_secs();
        }
        LogFormat::Json => {
            env_builder.format(|buf, record| {
                use std::io::Write;
                writeln!(
                    buf,
                    "{{\"ts\":{},\"lvl\":\"{}\",\"msg\":\"{}\"}}",
                    buf.timestamp(),
                    record.level(),
                    record.args()
                )
            });
        }
    }

    let pb = if cli.progress {
        let logger = env_builder.build();
        LogWrapper::new(multi.clone(), logger).try_init()?;
        Some(&multi)
    } else {
        env_builder.try_init()?;
        None
    };

    match cli.command {
        Commands::Decode(ref args) => cmd_decode(args, &cli, pb)?,
        Commands::Info(ref args) => cmd_info(args, &cli, pb)?,
    }

    Ok(())
}
