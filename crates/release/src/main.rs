//! `ts-release`: the release pipeline's command-line face.
//!
//! `keygen` and `sign*` run where the private key lives — the closed release
//! pipeline, never this repository's CI. `manifest`, `verify` and
//! `verify-plugin` run anywhere; they are the same code `token-station-cli
//! upgrade` uses, so a by-hand audit and the built-in one cannot disagree.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use token_station_release as release;

#[derive(Parser)]
#[command(
    name = "ts-release",
    version,
    about = "Sign and verify token-station releases"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate a signing keypair; the seed file must stay offline.
    Keygen {
        /// Directory for `release-signing.key` (seed, hex) and `.pub`.
        #[arg(long, default_value = ".")]
        out: PathBuf,
    },
    /// Hash artifacts into a release manifest.
    Manifest {
        /// The released version, e.g. `0.1.0`.
        #[arg(long)]
        version: String,
        /// Manifest timestamp; defaults to `$SOURCE_DATE_EPOCH`, then now.
        #[arg(long)]
        created: Option<u64>,
        #[arg(long)]
        out: PathBuf,
        /// The artifact files to hash.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Sign a manifest file's exact bytes; writes `<manifest>.sig`.
    Sign {
        #[arg(long)]
        key: PathBuf,
        manifest: PathBuf,
    },
    /// Verify a manifest against its signature and a public key.
    Verify {
        /// Hex public key, or a path to a file holding it.
        #[arg(long)]
        pubkey: String,
        manifest: PathBuf,
        /// Defaults to `<manifest>.sig`.
        #[arg(long)]
        signature: Option<PathBuf>,
    },
    /// Verify a Tauri updater payload against its encoded public key.
    VerifyUpdater {
        /// Base64 public key, or a path to a file holding it.
        #[arg(long)]
        pubkey: String,
        /// The updater payload, such as a macOS `.app.tar.gz`.
        artifact: PathBuf,
        /// Defaults to `<artifact>.sig`.
        #[arg(long)]
        signature: Option<PathBuf>,
    },
    /// Sign a plugin package directory; writes `signature.sig` inside it.
    SignPlugin {
        #[arg(long)]
        key: PathBuf,
        package: PathBuf,
    },
    /// Verify a plugin package against its embedded `signature.sig`.
    VerifyPlugin {
        #[arg(long)]
        pubkey: String,
        package: PathBuf,
    },
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
        Command::Keygen { out } => {
            let key = release::keygen()?;
            let seed_path = out.join("release-signing.key");
            let public_path = out.join("release-signing.pub");
            if seed_path.exists() {
                return Err(format!(
                    "`{}` already exists; refusing to overwrite a signing key",
                    seed_path.display()
                ));
            }
            write_private(&seed_path, &release::hex(key.as_bytes()))?;
            let public = release::hex(key.verifying_key().as_bytes());
            std::fs::write(&public_path, format!("{public}\n"))
                .map_err(|error| format!("write `{}`: {error}", public_path.display()))?;
            eprintln!("seed written to {} — keep it offline", seed_path.display());
            println!("{public}");
            Ok(())
        }
        Command::Manifest {
            version,
            created,
            out,
            files,
        } => {
            let created = created
                .or_else(|| {
                    std::env::var("SOURCE_DATE_EPOCH")
                        .ok()
                        .and_then(|raw| raw.parse().ok())
                })
                .unwrap_or_else(|| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |epoch| epoch.as_secs())
                });
            let paths: Vec<&std::path::Path> = files.iter().map(PathBuf::as_path).collect();
            let manifest = release::ReleaseManifest::for_files(&version, created, &paths)?;
            let mut rendered =
                serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?;
            rendered.push('\n');
            std::fs::write(&out, rendered)
                .map_err(|error| format!("write `{}`: {error}", out.display()))?;
            eprintln!(
                "{} artifact(s) -> {}",
                manifest.artifacts.len(),
                out.display()
            );
            Ok(())
        }
        Command::Sign { key, manifest } => {
            let key = release::load_signing_key(&key)?;
            let bytes = std::fs::read(&manifest)
                .map_err(|error| format!("read `{}`: {error}", manifest.display()))?;
            release::ReleaseManifest::parse(&bytes)?;
            let signature = release::sign_bytes(&key, &bytes);
            let out = signature_path(&manifest);
            std::fs::write(&out, format!("{signature}\n"))
                .map_err(|error| format!("write `{}`: {error}", out.display()))?;
            eprintln!("signature -> {}", out.display());
            Ok(())
        }
        Command::Verify {
            pubkey,
            manifest,
            signature,
        } => {
            let key = release::parse_public_key(&read_key_argument(&pubkey)?)?;
            let bytes = std::fs::read(&manifest)
                .map_err(|error| format!("read `{}`: {error}", manifest.display()))?;
            release::ReleaseManifest::parse(&bytes)?;
            let signature_file = signature.unwrap_or_else(|| signature_path(&manifest));
            let signature = std::fs::read_to_string(&signature_file)
                .map_err(|error| format!("read `{}`: {error}", signature_file.display()))?;
            release::verify_bytes(&key, &bytes, &signature)?;
            println!("ok: manifest verifies");
            Ok(())
        }
        Command::VerifyUpdater {
            pubkey,
            artifact,
            signature,
        } => verify_updater_command(&pubkey, &artifact, signature),
        Command::SignPlugin { key, package } => {
            let key = release::load_signing_key(&key)?;
            let signature = release::sign_plugin_package(&key, &package)?;
            let out = package.join(release::PLUGIN_SIGNATURE_FILE);
            std::fs::write(&out, format!("{signature}\n"))
                .map_err(|error| format!("write `{}`: {error}", out.display()))?;
            eprintln!("signature -> {}", out.display());
            Ok(())
        }
        Command::VerifyPlugin { pubkey, package } => {
            let key = release::parse_public_key(&read_key_argument(&pubkey)?)?;
            release::verify_plugin_package(&key, &package)?;
            println!("ok: plugin package verifies");
            Ok(())
        }
    }
}

fn verify_updater_command(
    pubkey: &str,
    artifact: &std::path::Path,
    signature: Option<PathBuf>,
) -> Result<(), String> {
    let public_key = read_key_argument(pubkey)?;
    let bytes = std::fs::read(artifact)
        .map_err(|error| format!("read `{}`: {error}", artifact.display()))?;
    let signature_file = signature.unwrap_or_else(|| signature_path(artifact));
    let signature = std::fs::read_to_string(&signature_file)
        .map_err(|error| format!("read `{}`: {error}", signature_file.display()))?;
    release::verify_updater_artifact(&public_key, &bytes, &signature)?;
    println!("ok: updater artifact verifies");
    Ok(())
}

fn signature_path(manifest: &std::path::Path) -> PathBuf {
    let mut name = manifest.file_name().unwrap_or_default().to_os_string();
    name.push(".sig");
    manifest.with_file_name(name)
}

/// `--pubkey` accepts the hex itself or a file holding it.
fn read_key_argument(argument: &str) -> Result<String, String> {
    let path = std::path::Path::new(argument);
    if path.exists() {
        std::fs::read_to_string(path)
            .map(|text| text.trim().to_owned())
            .map_err(|error| format!("read `{}`: {error}", path.display()))
    } else {
        Ok(argument.to_owned())
    }
}

/// The seed file is written owner-read-only where the platform can say so.
fn write_private(path: &std::path::Path, contents: &str) -> Result<(), String> {
    std::fs::write(path, format!("{contents}\n"))
        .map_err(|error| format!("write `{}`: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("chmod `{}`: {error}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    #[test]
    fn the_command_tree_is_coherent() {
        super::Cli::command().debug_assert();
    }
}
