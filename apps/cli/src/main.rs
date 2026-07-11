//! The command line: `serve` plus the management surface (C1#6).
//!
//! Parsing lives here; every operation lives in the library (`manage`,
//! `stats`, `gateway`), where the tests bite it. Command results go to
//! stdout; operational chatter and errors go to stderr, so `stats | column`
//! composes.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand, ValueEnum};
use token_station_cli::config::ClientConfig;
use token_station_cli::filelog::{FileLog, Recorders};
use token_station_cli::gateway::Gateway;
use token_station_cli::store::SqliteStore;
use token_station_cli::{manage, plugins, secrets, server, stats, upgrade, virtual_key};
use token_station_metrics::Recorder;

#[derive(Parser)]
#[command(
    name = "token-station-cli",
    version,
    about = "A loopback LLM proxy: local routing, BYOK, content never leaves your machine"
)]
struct Cli {
    /// The configuration file every command reads (and the editing ones
    /// rewrite atomically).
    #[arg(long, global = true, default_value = "token-station.json")]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the loopback proxy.
    Serve,
    /// Store or delete upstream credentials in the OS keychain.
    #[command(subcommand)]
    Key(KeyCommand),
    /// List, add, remove, or probe the configured upstreams.
    #[command(subcommand)]
    Upstream(UpstreamCommand),
    /// Flip switches, or edit the whole file behind validation.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Inspect the plugin packages and the provider dialects they register.
    #[command(subcommand)]
    Plugin(PluginCommand),
    /// Inspect the routing table.
    #[command(subcommand)]
    Rule(RuleCommand),
    /// Aggregate the local metrics store: volume, errors, latency, tokens.
    Stats(StatsArgs),
    /// Check for a newer release (anonymously); download only on explicit
    /// confirmation, and only if the signed manifest verifies.
    Upgrade {
        /// Skip the confirmation prompt (still verifies before keeping).
        #[arg(long)]
        yes: bool,
        /// Check and report only; never download.
        #[arg(long, conflicts_with = "yes")]
        check_only: bool,
    },
}

#[derive(Subcommand)]
enum KeyCommand {
    /// Store a credential (the value is read from stdin, never from argv).
    Set { upstream: String, slot: String },
    /// Delete a credential.
    Remove { upstream: String, slot: String },
}

#[derive(Subcommand)]
enum UpstreamCommand {
    /// The configured upstreams, their auth sources and models.
    List,
    /// Add an upstream to the configuration file.
    Add {
        /// Reference name, as pools will cite it (e.g. `openai_personal`).
        name: String,
        /// Provider dialect; a discovered plugin package must provide it
        /// (`plugin list` shows what does).
        #[arg(long)]
        provider: String,
        /// Base URL. No credentials in it — refused otherwise.
        #[arg(long)]
        base_url: String,
        /// Repeatable: `<model>[,tool][,vision][,json-schema][,ctx=N]`.
        #[arg(long = "model", required = true)]
        models: Vec<String>,
        /// Where the credential lives: `keyring` | `env:<VAR>` | `file:<PATH>`.
        /// Omit for open upstreams such as a local Ollama.
        #[arg(long)]
        auth: Option<String>,
        /// Credential slot name the provider adapter resolves.
        #[arg(long, default_value = "provider_api_key")]
        slot: String,
        /// Also append the models to this pool, so routing can reach them.
        #[arg(long)]
        pool: Option<String>,
    },
    /// Remove an upstream (refused while a pool still routes to it).
    Remove { name: String },
    /// Probe an upstream: one minimal real completion per declared model.
    Test {
        name: String,
        /// Probe only this model.
        #[arg(long)]
        model: Option<String>,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Flip a switch: `server.auth` or `data.metrics`.
    Set {
        switch: String,
        /// `on` or `off`.
        #[arg(value_parser = parse_on_off, action = clap::ArgAction::Set)]
        value: bool,
    },
    /// Open the file in $VISUAL/$EDITOR; applied only if the result validates.
    Edit,
}

#[derive(Subcommand)]
enum PluginCommand {
    /// Discovered packages, plus the dialect registry `upstream add` checks.
    List,
    /// Admit a package: run its conformance suite, copy it into the plugins
    /// directory, record the approval. The path holds manifest.json,
    /// adapter.wasm and fixtures/.
    Install { path: PathBuf },
    /// Delete an installed package and its approval (refused while an
    /// upstream still routes through it).
    Remove { name: String },
    /// One package in full: identity, trust, dialects, declared secrets.
    Info { name: String },
}

#[derive(Subcommand)]
enum RuleCommand {
    /// The routing table, layer by layer, in evaluation order.
    List,
}

#[derive(clap::Args)]
struct StatsArgs {
    /// Window: `all`, `<N>h`, or `<N>d`.
    #[arg(long, default_value = "24h")]
    since: String,
    /// Group by one dimension instead of totals only.
    #[arg(long, value_enum)]
    by: Option<GroupByArg>,
}

#[derive(Clone, Copy, ValueEnum)]
enum GroupByArg {
    Upstream,
    Model,
    Pool,
    Status,
}

impl From<GroupByArg> for stats::GroupBy {
    fn from(by: GroupByArg) -> Self {
        match by {
            GroupByArg::Upstream => Self::Upstream,
            GroupByArg::Model => Self::Model,
            GroupByArg::Pool => Self::Pool,
            GroupByArg::Status => Self::Status,
        }
    }
}

fn parse_on_off(raw: &str) -> Result<bool, String> {
    match raw {
        "on" | "true" => Ok(true),
        "off" | "false" => Ok(false),
        _ => Err(format!("`{raw}` is not on/off")),
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Serve => serve(&cli.config),
        Command::Key(command) => key(&command),
        Command::Upstream(UpstreamCommand::List) => {
            let config = load(&cli.config)?;
            print!("{}", manage::upstream_list(&config));
            Ok(())
        }
        Command::Upstream(UpstreamCommand::Add {
            name,
            provider,
            base_url,
            models,
            auth,
            slot,
            pool,
        }) => mutate(&cli.config, |config| {
            // Refuse an unresolvable dialect now, at the terminal where the
            // operator can act on it — not at the next `serve`.
            let registry = plugins::PluginRegistry::for_config(config)?;
            if registry.provider_binding(&provider).is_none() {
                return Err(format!(
                    "no plugin provides dialect `{provider}`; available: [{}] (scanned {})",
                    registry.provider_dialects().join(", "),
                    config.plugins.dir.display(),
                ));
            }
            manage::upstream_add(
                config,
                &manage::AddUpstream {
                    name: &name,
                    provider: &provider,
                    base_url: &base_url,
                    models: &models,
                    auth: auth.as_deref(),
                    slot: &slot,
                    pool: pool.as_deref(),
                },
            )
        }),
        Command::Upstream(UpstreamCommand::Remove { name }) => {
            mutate(&cli.config, |config| manage::upstream_remove(config, &name))
        }
        Command::Upstream(UpstreamCommand::Test { name, model }) => {
            probe(&cli.config, &name, model.as_deref())
        }
        Command::Config(ConfigCommand::Set { switch, value }) => mutate(&cli.config, |config| {
            manage::set_switch(config, &switch, value)
        }),
        Command::Config(ConfigCommand::Edit) => {
            let editor = std::env::var("VISUAL")
                .or_else(|_| std::env::var("EDITOR"))
                .map_err(|_| "config edit needs $VISUAL or $EDITOR set".to_owned())?;
            let summary = manage::edit(&cli.config, &editor)?;
            println!("{summary}");
            eprintln!("note: a running `serve` applies configuration changes on restart");
            Ok(())
        }
        Command::Plugin(PluginCommand::List) => {
            let config = load(&cli.config)?;
            let registry = plugins::PluginRegistry::for_config(&config)?;
            print!("{}", registry.render_list());
            Ok(())
        }
        Command::Plugin(PluginCommand::Install { path }) => {
            let config = load(&cli.config)?;
            println!("{}", plugins::install(&config, &path)?);
            eprintln!("note: a running `serve` applies plugin changes on restart");
            Ok(())
        }
        Command::Plugin(PluginCommand::Remove { name }) => {
            let config = load(&cli.config)?;
            println!("{}", plugins::remove(&config, &name)?);
            Ok(())
        }
        Command::Plugin(PluginCommand::Info { name }) => {
            let config = load(&cli.config)?;
            print!("{}", plugins::info(&config, &name)?);
            Ok(())
        }
        Command::Rule(RuleCommand::List) => {
            let config = load(&cli.config)?;
            print!("{}", manage::rule_list(&config));
            Ok(())
        }
        Command::Stats(args) => {
            let config = load(&cli.config)?;
            let window = stats::parse_since(&args.since)?;
            let cutoff = window.map(|window| now_ms().saturating_sub(window));
            let group_by = args.by.map(stats::GroupBy::from);
            let report = stats::collect(&config.data.dir.join("metrics.sqlite"), cutoff, group_by)?;
            print!("{}", stats::render(&report, group_by));
            Ok(())
        }
        Command::Upgrade { yes, check_only } => upgrade_command(yes, check_only),
    }
}

fn upgrade_command(yes: bool, check_only: bool) -> Result<(), String> {
    let release = upgrade::check(upgrade::DEFAULT_ENDPOINT)?;
    if !upgrade::is_newer(upgrade::CURRENT_VERSION, &release.tag_name) {
        println!(
            "up to date: {} (latest release is {})",
            upgrade::CURRENT_VERSION,
            release.tag_name
        );
        return Ok(());
    }

    println!(
        "new release {} (this build is {}): {}",
        release.tag_name,
        upgrade::CURRENT_VERSION,
        release.html_url
    );
    if check_only {
        return Ok(());
    }

    // Require explicit upgrade confirmation; nothing downloads without human approval.
    if !yes {
        eprint!("download and verify {}? [y/N] ", release.tag_name);
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .map_err(|error| format!("stdin: {error}"))?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            println!("not downloading");
            return Ok(());
        }
    }

    let path = upgrade::download_and_verify(
        &release,
        Path::new("."),
        upgrade::OFFICIAL_RELEASE_PUBKEY_HEX,
    )?;
    println!(
        "verified against the signed manifest: {}\nunpack it and replace this binary when ready — nothing was installed",
        path.display()
    );
    Ok(())
}

fn load(path: &Path) -> Result<ClientConfig, String> {
    ClientConfig::load(path).map_err(|error| error.to_string())
}

/// Load → edit → validate-and-save, atomically: a refused edit leaves the
/// file byte-for-byte as it was.
fn mutate(
    path: &Path,
    edit: impl FnOnce(&mut ClientConfig) -> Result<String, String>,
) -> Result<(), String> {
    let mut config = load(path)?;
    let summary = edit(&mut config)?;
    config.save(path).map_err(|error| error.to_string())?;
    println!("{summary}");
    eprintln!("note: a running `serve` applies configuration changes on restart");
    Ok(())
}

fn key(command: &KeyCommand) -> Result<(), String> {
    match command {
        // `key set` reads from stdin: a key in argv is a key in `ps` output
        // and shell history.
        KeyCommand::Set { upstream, slot } => {
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
            Ok(())
        }
        KeyCommand::Remove { upstream, slot } => {
            secrets::keyring_remove(upstream, slot)?;
            eprintln!("removed {upstream}/{slot} from the OS keychain");
            Ok(())
        }
    }
}

fn probe(config_path: &Path, name: &str, model: Option<&str>) -> Result<(), String> {
    let config = load(config_path)?;
    // No recorder: a probe is operator diagnostics, not served traffic, and it
    // must not skew `stats`.
    let gateway = Gateway::new(&config, Arc::new(token_station_metrics::NoopRecorder))?;

    let outcomes = gateway.probe(name, model)?;
    let mut failed = 0usize;
    for outcome in &outcomes {
        match &outcome.latency_ms {
            Ok(latency) => println!("{}: ok ({latency} ms)", outcome.model),
            Err(reason) => {
                failed += 1;
                println!("{}: FAIL — {reason}", outcome.model);
            }
        }
    }
    if failed > 0 {
        Err(format!("{failed} of {} probe(s) failed", outcomes.len()))
    } else {
        Ok(())
    }
}

fn serve(config_path: &Path) -> Result<(), String> {
    let config = load(config_path)?;
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

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |epoch| {
            u64::try_from(epoch.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    /// clap's own coherence check: conflicting flags, broken defaults and the
    /// like fail here instead of at first use in the field.
    #[test]
    fn the_command_tree_is_coherent() {
        super::Cli::command().debug_assert();
    }
}
