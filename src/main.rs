use clap::Parser;
use cmdline::Cmdline;
use config::Config;
use figment::providers::{Format, Toml};
use server::{App, Template};
use tracing_subscriber::{Layer, filter::EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use crate::landlock::setup_landlock;

mod cmdline;
mod config;
mod landlock;
mod server;

fn init_logging() {
    let console_subscriber = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_file(true)
        .with_thread_names(true)
        .with_line_number(true)
        .with_target(false)
        .with_ansi(true)
        .with_filter(EnvFilter::new(format!(
            "info,{}",
            std::env::var("YADEX_LOGLEVEL").unwrap_or_default()
        )));
    tracing_subscriber::registry()
        .with(console_subscriber)
        .init();
}

fn main() -> color_eyre::Result<()> {
    init_logging();
    color_eyre::install()?;
    let cmdline = Cmdline::parse();
    tracing::info!("cmdline: {:?}", cmdline);
    let config: Config = figment::Figment::new()
        .merge(Toml::file(&cmdline.config))
        .extract()?;

    if config.service.security == config::Security::Landlock {
        setup_landlock(&cmdline, &config)?;
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(run(cmdline, config))
}

async fn run(cmdline: Cmdline, config: Config) -> color_eyre::Result<()> {
    let template = match config.service.template_index {
        true => Template::from_config(&cmdline.config, config.template)?,
        false => Template::default(),
    };
    let listener =
        tokio::net::TcpListener::bind((config.network.address, config.network.port)).await?;
    tracing::info!("Yadex listening on {}", listener.local_addr()?);

    App::serve(config.service, listener, template).await?;
    Ok(())
}
