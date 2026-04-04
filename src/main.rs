mod app;
mod bluetooth;
mod cli;
mod config;
mod device;
mod error;
mod router;
mod virtual_keyboard;
mod virtual_mouse;

use app::run;
use clap::Parser;
use cli::Cli;
use error::AppError;
use fern::colors::{Color, ColoredLevelConfig};
use log::LevelFilter;

fn init_logging(debug: bool) -> Result<(), AppError> {
    let level = if debug {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };

    let colors = ColoredLevelConfig::new()
        .error(Color::Red)
        .warn(Color::Yellow)
        .info(Color::Green)
        .debug(Color::Blue)
        .trace(Color::BrightBlack);

    fern::Dispatch::new()
        .level(level)
        .format(move |out, message, record| {
            out.finish(format_args!(
                "[{}] {}",
                colors.color(record.level()),
                message
            ))
        })
        .chain(std::io::stderr())
        .apply()?;

    Ok(())
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();

    if let Err(err) = init_logging(cli.debug) {
        eprintln!("[ERROR] {err}");
        return std::process::ExitCode::FAILURE;
    }

    match run(cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            log::error!("{err}");
            std::process::ExitCode::FAILURE
        }
    }
}
