//! Cross-platform owner-only filesystem primitives for Token Station state.
//!
//! Unix keeps the established `0700` directory / `0600` file contract.
//! Windows uses a protected DACL containing exactly one full-control ACE for
//! the object owner, and verifies that ACL before sensitive bytes are renamed
//! into their final path.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Creates or hardens a real directory for owner-only state.
pub fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) || !metadata.is_dir() {
                return Err(invalid("private directory is not a real directory"));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path)?;
        }
        Err(error) => return Err(error),
    }
    harden_path(path)?;
    verify_private_dir(path)
}

/// Verifies that a path is a real owner-only directory.
pub fn verify_private_dir(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(invalid("private directory is not a real directory"));
    }
    verify_platform_private(path, &metadata, PrivateKind::Directory)
}

/// Verifies that a path is a real owner-only regular file.
pub fn verify_private_file(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err(invalid("private file is not a real regular file"));
    }
    verify_platform_private(path, &metadata, PrivateKind::File)
}

/// Hardens an existing regular file and verifies the resulting protection.
pub fn harden_private_file(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err(invalid("private file is not a real regular file"));
    }
    harden_path(path)?;
    verify_private_file(path)
}

/// Creates a new owner-only file without ever writing sensitive bytes before
/// the final path is protected. Returns `false` when another process won the
/// create race; the existing file is verified before that result is returned.
pub fn create_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("private target has no parent"))?;
    ensure_real_parent_dir(parent)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            verify_private_file(path)?;
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    let result = (|| {
        harden_private_file(path)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        verify_private_file(path)?;
        sync_parent(parent)
    })();
    if let Err(error) = result {
        std::fs::remove_file(path).ok();
        return Err(error);
    }
    Ok(true)
}

/// Atomically replaces a file without ever publishing the new sensitive bytes
/// before the temporary file has passed the owner-only protection check.
pub fn write_atomic_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("private target has no parent"))?;
    ensure_real_parent_dir(parent)?;
    match std::fs::symlink_metadata(path) {
        Ok(_) => harden_private_file(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
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

        harden_private_file(&temporary)?;
        atomic_replace(&temporary, path)?;
        verify_private_file(path)?;
        sync_parent(parent)
    })();
    if result.is_err() {
        std::fs::remove_file(&temporary).ok();
    }
    result
}

fn invalid(message: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message)
}

fn ensure_real_parent_dir(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) || !metadata.is_dir() {
                return Err(invalid("private file parent is not a real directory"));
            }
            // The caller may intentionally place a 0600 file in an existing
            // shared/system directory. Never chmod or rewrite that directory's
            // ACL as a side effect of saving the file itself.
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ensure_private_dir(path),
        Err(error) => Err(error),
    }
}

#[derive(Clone, Copy)]
enum PrivateKind {
    Directory,
    File,
}

#[cfg(unix)]
fn harden_path(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::symlink_metadata(path)?;
    let mode = if metadata.is_dir() { 0o700 } else { 0o600 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(windows)]
fn harden_path(path: &Path) -> std::io::Result<()> {
    windows::apply_owner_only_dacl(path)
}

#[cfg(not(any(unix, windows)))]
fn harden_path(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn verify_platform_private(
    _path: &Path,
    metadata: &std::fs::Metadata,
    kind: PrivateKind,
) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = metadata.permissions().mode() & 0o777;
    let expected = match kind {
        PrivateKind::Directory => 0o700,
        PrivateKind::File => 0o600,
    };
    if mode != expected {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "private path permissions are not owner-only",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn verify_platform_private(
    path: &Path,
    _metadata: &std::fs::Metadata,
    _kind: PrivateKind,
) -> std::io::Result<()> {
    windows::verify_owner_only_dacl(path)
}

#[cfg(not(any(unix, windows)))]
fn verify_platform_private(
    _path: &Path,
    _metadata: &std::fs::Metadata,
    _kind: PrivateKind,
) -> std::io::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn unique_temporary_path(path: &Path) -> std::io::Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| invalid("invalid private filename"))?;
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

#[cfg(not(windows))]
fn atomic_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::rename(source, target)
}

#[cfg(windows)]
fn atomic_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let target = target
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    // SAFETY: both owned buffers are NUL-terminated and remain alive for the
    // synchronous Win32 call.
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn sync_parent(parent: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let result = std::fs::File::open(parent)?.sync_all();
        #[cfg(target_os = "macos")]
        if let Err(error) = &result {
            // APFS-backed system temporary directories can reject directory
            // fsync with EPERM/EINVAL even though the atomic rename succeeded.
            // File contents were already fsynced; this is a durability
            // limitation of that mount, not a permissions failure.
            if matches!(error.raw_os_error(), Some(1) | Some(22) | Some(45)) {
                return Ok(());
            }
        }
        result
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
        Ok(())
    }
}

#[cfg(windows)]
mod windows {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation,
        GetSecurityDescriptorControl, GetTokenInformation, IsValidSid, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
        SetFileSecurityW, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    pub(super) fn apply_owner_only_dacl(path: &Path) -> std::io::Result<()> {
        let current_user = current_user_sid()?;
        let sid = sid_string(current_user.as_ptr())?;
        let sddl = format!("O:{sid}D:P(A;;FA;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: the SDDL buffer is NUL-terminated. On success Windows owns
        // the allocation contract and we release it exactly once with
        // LocalFree below.
        let created = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if created == 0 || descriptor.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let encoded = wide(path);
        // SAFETY: both pointers remain valid for this synchronous call.
        let applied = unsafe {
            SetFileSecurityW(
                encoded.as_ptr(),
                OWNER_SECURITY_INFORMATION
                    | DACL_SECURITY_INFORMATION
                    | PROTECTED_DACL_SECURITY_INFORMATION,
                descriptor,
            )
        };
        // SAFETY: `descriptor` came from the conversion API above.
        unsafe { LocalFree(descriptor.cast()) };
        if applied == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub(super) fn verify_owner_only_dacl(path: &Path) -> std::io::Result<()> {
        let encoded = wide(path);
        let mut owner = std::ptr::null_mut();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: output pointers are valid and the returned descriptor is
        // released with LocalFree after all borrowed owner/DACL pointers are
        // finished.
        let status = unsafe {
            GetNamedSecurityInfoW(
                encoded.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        if status != 0 {
            return Err(std::io::Error::from_raw_os_error(status.cast_signed()));
        }
        let current_user = current_user_sid();
        let verified = current_user.and_then(|current_user| {
            verify_descriptor(descriptor, owner, dacl, current_user.as_ptr())
        });
        // SAFETY: descriptor was allocated by GetNamedSecurityInfoW.
        unsafe { LocalFree(descriptor.cast()) };
        verified
    }

    fn verify_descriptor(
        descriptor: PSECURITY_DESCRIPTOR,
        owner: *mut c_void,
        dacl: *mut ACL,
        current_user: PSID,
    ) -> std::io::Result<()> {
        if descriptor.is_null() || owner.is_null() || dacl.is_null() {
            return Err(denied("private path has no owner or DACL"));
        }
        let mut control = 0_u16;
        let mut revision = 0_u32;
        // SAFETY: descriptor points to a live self-relative security
        // descriptor for the duration of this function.
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if control & SE_DACL_PROTECTED == 0 {
            return Err(denied("private path DACL still inherits permissions"));
        }
        // SAFETY: both SIDs are live for the duration of this call.
        if unsafe { EqualSid(owner, current_user) } == 0 {
            return Err(denied("private path is not owned by the current user"));
        }

        let mut info = ACL_SIZE_INFORMATION {
            AceCount: 0,
            AclBytesInUse: 0,
            AclBytesFree: 0,
        };
        // SAFETY: DACL and output buffer are valid for the synchronous call.
        if unsafe {
            GetAclInformation(
                dacl,
                (&raw mut info).cast(),
                u32::try_from(std::mem::size_of::<ACL_SIZE_INFORMATION>())
                    .expect("ACL info size fits u32"),
                AclSizeInformation,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        if info.AceCount != 1 {
            return Err(denied("private path DACL has unexpected principals"));
        }

        let mut ace = std::ptr::null_mut();
        // SAFETY: the verified DACL has one ACE at index zero.
        if unsafe { GetAce(dacl, 0, &mut ace) } == 0 || ace.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: GetAce returned a pointer to the first ACE. The SDDL we
        // apply requires an ACCESS_ALLOWED_ACE, checked before reading SidStart.
        let allowed = unsafe { &*(ace.cast::<ACCESS_ALLOWED_ACE>()) };
        const ACCESS_ALLOWED_ACE_TYPE_VALUE: u8 = 0;
        if allowed.Header.AceType != ACCESS_ALLOWED_ACE_TYPE_VALUE {
            return Err(denied("private path DACL contains a non-allow ACE"));
        }
        let sid = (&raw const allowed.SidStart).cast::<c_void>().cast_mut();
        // SAFETY: owner and ACE SID pointers are valid within descriptor/DACL.
        if unsafe { EqualSid(current_user, sid) } == 0 {
            return Err(denied("private path DACL grants a non-owner principal"));
        }
        const FILE_ALL_ACCESS_MASK: u32 = 0x001F_01FF;
        if allowed.Mask & FILE_ALL_ACCESS_MASK != FILE_ALL_ACCESS_MASK {
            return Err(denied("private path owner lacks full control"));
        }
        Ok(())
    }

    struct TokenHandle(HANDLE);

    impl Drop for TokenHandle {
        fn drop(&mut self) {
            // SAFETY: the handle was returned by OpenProcessToken and remains
            // owned by this guard.
            unsafe { CloseHandle(self.0) };
        }
    }

    struct SidBuffer {
        storage: Vec<usize>,
    }

    impl SidBuffer {
        fn as_ptr(&self) -> PSID {
            // TOKEN_USER is the first value in the aligned buffer and its SID
            // points into that same buffer, which this owner keeps alive.
            let token_user = self.storage.as_ptr().cast::<TOKEN_USER>();
            // SAFETY: GetTokenInformation initialized a complete TOKEN_USER.
            unsafe { (*token_user).User.Sid }
        }
    }

    fn current_user_sid() -> std::io::Result<SidBuffer> {
        let mut raw_token: HANDLE = std::ptr::null_mut();
        // SAFETY: output handle is valid and closed by TokenHandle below.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw_token) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let token = TokenHandle(raw_token);
        let mut required = 0_u32;
        // The first call intentionally obtains the required byte count.
        unsafe { GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut required) };
        if required == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let word = std::mem::size_of::<usize>();
        let words = usize::try_from(required)
            .map_err(std::io::Error::other)?
            .div_ceil(word);
        let mut storage = vec![0_usize; words];
        // SAFETY: the aligned allocation has at least `required` writable bytes.
        if unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                storage.as_mut_ptr().cast(),
                required,
                &mut required,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let sid = SidBuffer { storage };
        // SAFETY: SID points into the live TOKEN_USER buffer.
        if unsafe { IsValidSid(sid.as_ptr()) } == 0 {
            return Err(super::invalid("current process token has an invalid SID"));
        }
        Ok(sid)
    }

    fn sid_string(sid: PSID) -> std::io::Result<String> {
        let mut value = std::ptr::null_mut();
        // SAFETY: SID is valid and the output is freed with LocalFree.
        if unsafe { ConvertSidToStringSidW(sid, &mut value) } == 0 || value.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let mut length = 0;
        // SAFETY: ConvertSidToStringSidW returns a NUL-terminated string.
        while unsafe { *value.add(length) } != 0 {
            length += 1;
        }
        // SAFETY: `length` was measured within the returned allocation.
        let result = String::from_utf16(unsafe { std::slice::from_raw_parts(value, length) })
            .map_err(std::io::Error::other);
        // SAFETY: value was allocated by ConvertSidToStringSidW.
        unsafe { LocalFree(value.cast()) };
        result
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    fn denied(message: &'static str) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random).expect("system randomness");
        std::env::temp_dir().join(format!(
            "token-station-private-fs-{}-{}",
            std::process::id(),
            random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ))
    }

    #[test]
    fn atomic_writer_hardens_directory_and_file() {
        let root = scratch();
        let target = root.join("state.json");
        write_atomic_private(&target, b"first").expect("first write");
        write_atomic_private(&target, b"second").expect("replacement");
        assert_eq!(std::fs::read(&target).expect("read state"), b"second");
        verify_private_dir(&root).expect("private directory verifies");
        verify_private_file(&target).expect("private file verifies");
        assert!(std::fs::read_dir(&root).expect("list root").all(|entry| {
            !entry
                .expect("directory entry")
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn real_path_type_is_required() {
        let root = scratch();
        std::fs::create_dir_all(&root).expect("scratch directory");
        let regular = root.join("regular");
        std::fs::write(&regular, b"value").expect("regular file");
        assert_eq!(
            ensure_private_dir(&regular)
                .expect_err("file is not a directory")
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert_eq!(
            verify_private_file(&root)
                .expect_err("directory is not a file")
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn create_private_file_does_not_replace_an_existing_value() {
        let root = scratch();
        let target = root.join("virtual-key");
        assert!(create_private_file(&target, b"first").expect("first create"));
        assert!(!create_private_file(&target, b"second").expect("existing file"));
        assert_eq!(std::fs::read(&target).expect("read key"), b"first");
        verify_private_file(&target).expect("existing file remains private");
        std::fs::remove_dir_all(root).ok();
    }
}
