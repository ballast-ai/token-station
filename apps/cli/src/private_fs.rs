use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) fn write_atomic_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "target has no parent")
    })?;
    ensure_private_dir(parent)?;
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "private target is not a real regular file",
            ));
        }
        make_private(path)?;
    }

    let temporary = unique_temporary_path(path)?;
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);

        let old = if path.exists() {
            let old = unique_temporary_path(path)?;
            std::fs::rename(path, &old)?;
            Some(old)
        } else {
            None
        };
        if let Err(error) = std::fs::rename(&temporary, path) {
            if let Some(old) = &old {
                let _ = std::fs::rename(old, path);
            }
            return Err(error);
        }
        if let Some(old) = old {
            std::fs::remove_file(old)?;
        }
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        std::fs::remove_file(&temporary).ok();
    }
    result
}

pub(crate) fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
    let mut created = false;
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "private directory is not a real directory",
            ));
        }
    } else {
        std::fs::create_dir_all(path)?;
        created = true;
    }
    #[cfg(unix)]
    if created {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    let _ = created;
    Ok(())
}

pub(crate) fn make_private(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let _ = path;
    Ok(())
}

fn unique_temporary_path(path: &Path) -> std::io::Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| std::io::Error::other("invalid private filename"))?;
    for _ in 0..16 {
        let mut random = [0_u8; 12];
        getrandom::fill(&mut random).map_err(std::io::Error::other)?;
        let suffix = random.iter().fold(String::new(), |mut output, byte| {
            use std::fmt::Write as _;
            let _ = write!(output, "{byte:02x}");
            output
        });
        let candidate = path.with_file_name(format!(".{name}.{suffix}.tmp"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "cannot allocate private temporary file",
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn private_writer_is_atomic_and_owner_only() {
        let root = std::env::temp_dir().join(format!("ts-private-fs-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let target = root.join("state.json");
        super::write_atomic_private(&target, b"first").unwrap();
        super::write_atomic_private(&target, b"second").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"second");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(root).ok();
    }
}
