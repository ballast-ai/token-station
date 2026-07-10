use std::process::ExitCode;
use std::sync::Arc;

use token_station_cli::config::ClientConfig;
use token_station_cli::filelog::{FileLog, Recorders};
use token_station_cli::gateway::Gateway;
use token_station_cli::store::SqliteStore;
use token_station_cli::{secrets, server, virtual_key};
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

const USAGE: &str = "usage: token-station-cli serve [--config <path>]
       token-station-cli key set <upstream> <slot>      (reads the key from stdin)
       token-station-cli key remove <upstream> <slot>";

fn run() -> Result<(), String> {
    // Hand-rolled until C1#6 builds the real management surface.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();

    let config_path = match words.as_slice() {
        ["serve"] => "token-station.json".to_owned(),
        ["serve", "--config", path] => (*path).to_owned(),
        // `key set` reads from stdin: a key in argv is a key in `ps` output
        // and shell history.
        ["key", "set", upstream, slot] => {
            eprintln!("paste the key for {upstream}/{slot}, then press Enter:");
            let mut line = String::new();
            std::io::stdin()
                .read_line(&mut line)
                .map_err(|error| format!("stdin: {error}"))?;
            let value = line.trim();
            if value.is_empty() {
                return Err("no key on stdin; nothing stored".to_owned());
            }
            secrets::keyring_set(upstream, slot, value)?;
            eprintln!("stored in the OS keychain as {upstream}/{slot}");
            return Ok(());
        }
        ["key", "remove", upstream, slot] => {
            secrets::keyring_remove(upstream, slot)?;
            eprintln!("removed {upstream}/{slot} from the OS keychain");
            return Ok(());
        }
        _ => return Err(USAGE.to_owned()),
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

    // Authentication is on by default; the virtual key exists before the port does.
    let key = if config.server.auth {
        let (key, created) = virtual_key::load_or_create(&config.data.dir)?;
        if created {
            // Printed exactly once, at creation. Later startups say where it
            // lives instead, because startup output tends to end up in logs.
            eprintln!("local virtual key (created now, shown once): {key}");
        } else {
            eprintln!(
                "auth on; virtual key at {}",
                config.data.dir.join("virtual-key").display()
            );
        }
        Some(std::sync::Arc::<str>::from(key.as_str()))
    } else {
        eprintln!("warning: server.auth is off; any local process can use this proxy");
        None
    };

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
        server::serve(
            server::AppState {
                gateway,
                virtual_key: key,
            },
            listener,
        )
        .await
        .map_err(|error| format!("server: {error}"))
    })
}
