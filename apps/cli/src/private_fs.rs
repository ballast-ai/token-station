use std::path::Path;

pub(crate) use token_station_private_fs::{ensure_private_dir, write_atomic_private};

pub(crate) fn make_private(path: &Path) -> std::io::Result<()> {
    token_station_private_fs::harden_private_file(path)
}
