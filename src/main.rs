use anyhow::Result;
use clap::Parser as ClapParser;
use cli::command::{Cli, Commands, LogFormat};
use cli::decode::cmd_decode;
use cli::info::cmd_info;
use indicatif::MultiProgress;
use indicatif_log_bridge::LogWrapper;
use log::info;

// The format writers model more of CAF, Wave64 and their integer encodings
// than the CLI currently emits.
#[allow(dead_code)]
mod byteorder;
#[allow(dead_code)]
mod caf;
mod cli;
mod damf;
mod exit;
mod input;
mod json;
pub(crate) mod timestamp;
#[allow(dead_code)]
mod wav;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::from(exit::SUCCESS as u8),
        Err(error) => {
            log::error!("{error:#}");
            std::process::ExitCode::from(exit::code_for(&error) as u8)
        }
    }
}

fn run() -> Result<()> {
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
                    "{{\"ts\":{},\"lvl\":{},\"msg\":{}}}",
                    json::escape(&buf.timestamp().to_string()),
                    json::escape(record.level().as_str()),
                    json::escape(&record.args().to_string())
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

    info!("{}", cli::command::VERSION_INFO);

    match cli.command {
        Commands::Decode(ref args) => cmd_decode(args, &cli, pb)?,
        Commands::Info(ref args) => cmd_info(args, &cli, pb)?,
    }

    Ok(())
}
