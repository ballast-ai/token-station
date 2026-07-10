use std::process::ExitCode;
use std::sync::Arc;

use token_station_cli::config::ClientConfig;
use token_station_cli::filelog::{FileLog, Recorders};
use token_station_cli::gateway::Gateway;
use token_station_cli::server;
use token_station_cli::store::SqliteStore;
use token_station_metrics::Recorder;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    // `serve [--config <path>]`. The real management surface is C1#6; until
    // then the binary does one thing and does not pretend otherwise.
    let mut args = std::env::args().skip(1);
    let command = args.next();
    let config_path = match (command.as_deref(), args.next().as_deref(), args.next()) {
        (Some("serve"), None, None) => "token-station.json".to_owned(),
        (Some("serve"), Some("--config"), Some(path)) => path,
        _ => {
            return Err("usage: token-station-cli serve [--config <path>]".to_owned());
        }
    };

    let config = ClientConfig::load(std::path::Path::new(&config_path))
        .map_err(|error| error.to_string())?;
    let listen = config.server.listen.clone();

    // The file log is always written; the metrics store is on unless the
    // operator turned it off. Both hold the same content-free record.
    let mut sinks: Vec<Box<dyn Recorder>> = vec![Box::new(FileLog::open(&config.data.dir)?)];
    if config.data.metrics {
        sinks.push(Box::new(SqliteStore::open(
            &config.data.dir.join("metrics.sqlite"),
        )?));
    }
    let gateway = Arc::new(Gateway::new(&config, Arc::new(Recorders(sinks)))?);

    eprintln!(
        "token-station listening on http://{listen} — {} upstream(s), {} model(s) in catalog",
        config.upstreams.len(),
        gateway.catalog_size(),
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("tokio runtime: {error}"))?;

    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(&listen)
            .await
            .map_err(|error| format!("bind {listen}: {error}"))?;
        server::serve(gateway, listener)
            .await
            .map_err(|error| format!("server: {error}"))
    })
}
