//! Writing a private key to disk without ever exposing or destroying it.

use std::path::Path;

/// Writes a PEM-encoded private key to `path`, restricted to the current user.
///
/// The key is written to a sibling `<name>.tmp`, restricted, and only then renamed over
/// `path`, so an existing good key is never truncated or removed by a failed write or a
/// failed ACL step; on any failure only the temporary file is deleted.
///
/// # What "restricted" means on each platform
///
/// - **Unix**: the temporary file is `open(2)`-ed with mode `0o600` from the very first
///   syscall that creates it ([`std::os::unix::fs::OpenOptionsExt::mode`]) -- there is no
///   window at all where it exists at a looser mode, since the mode is part of creation,
///   not a separate `chmod` afterward. The rename keeps the mode.
/// - **Windows**: `std` has no equivalent "create with an ACL already attached" call, so
///   this creates the temporary file EMPTY first, restricts its ACL via `icacls`
///   ([`restrict_key_file`]), and only then writes the actual key bytes. The file's mere
///   (empty) existence is briefly visible at the parent directory's inherited
///   permissions, but the private key content itself is never written before the ACL is
///   locked down. The rename carries the restricted ACL over to `path`.
///
/// Shared by `apps/indicatrix-worker` and `apps/indicatrix-cut` (via
/// [`crate::enroll::claim`]'s `client.key`). Returns a plain `String` rather than
/// [`super::TlsError`] since the failure modes here (directory creation, an external
/// `icacls`/`whoami` process) aren't TLS or certificate-parsing errors.
///
/// # Errors
///
/// A human-readable message if the parent directory can't be created, the file can't be
/// created/written/renamed, or (Windows) its permissions can't be restricted.
pub fn write_private_key_pem(path: &Path, pem: &str) -> Result<(), String> {
    #[cfg(windows)]
    let restrict = restrict_key_file;
    #[cfg(not(windows))]
    let restrict = |_: &Path| -> Result<(), String> { Ok(()) };
    write_private_key_with(path, pem, restrict)
}

/// [`write_private_key_pem`] with the permission-restricting step injected, so a failing
/// step can be exercised on every platform.
fn write_private_key_with(
    path: &Path,
    pem: &str,
    restrict: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    let tmp = temp_sibling(path)?;
    let result = write_restricted(&tmp, pem, restrict).and_then(|()| {
        std::fs::rename(&tmp, path).map_err(|e| {
            format!(
                "could not move {} over {}: {e}",
                tmp.display(),
                path.display()
            )
        })
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// `<name>.tmp` next to `path`.
fn temp_sibling(path: &Path) -> Result<std::path::PathBuf, String> {
    let mut name = path
        .file_name()
        .ok_or_else(|| format!("{} has no file name", path.display()))?
        .to_os_string();
    name.push(".tmp");
    Ok(path.with_file_name(name))
}

/// Creates `path` empty and private, runs `restrict`, and only then writes `pem`.
fn write_restricted(
    path: &Path,
    pem: &str,
    restrict: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    create_empty_private_file(path)?;
    restrict(path).map_err(|e| {
        format!(
            "created {} but could not restrict its permissions ({e}) -- the (empty, key-less) file has been \
             removed rather than left behind world-readable; fix the underlying issue and retry",
            path.display()
        )
    })?;
    std::fs::write(path, pem).map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// Unix half of the private-file creation: mode `0o600` in the same `open(2)` call that
/// creates the file (`0o600` has no group/other bits to begin with, so the process
/// `umask` -- which can only CLEAR bits -- can never widen it).
#[cfg(not(windows))]
fn create_empty_private_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map(|_| ())
        .map_err(|e| format!("could not create {}: {e}", path.display()))
}

/// Windows half: create (or truncate) empty; the ACL is restricted afterwards by the
/// caller, before any key byte is written.
#[cfg(windows)]
fn create_empty_private_file(path: &Path) -> Result<(), String> {
    std::fs::File::create(path)
        .map(|_| ())
        .map_err(|e| format!("could not create {}: {e}", path.display()))
}

/// Restricts `path` to the current user (plus `SYSTEM` and `Administrators`) via
/// `icacls` -- Windows has no `chmod`, so this needs an ACL instead.
#[cfg(windows)]
fn restrict_key_file(path: &Path) -> Result<(), String> {
    let user = current_user_account()?;
    let path_str = path.to_str().ok_or_else(|| {
        format!(
            "{}: path is not valid Unicode, icacls requires a printable path",
            path.display()
        )
    })?;

    let status = std::process::Command::new("icacls")
        .arg(path_str)
        // Drop inherited ACEs (parent dirs typically grant `Users` read access) before
        // granting anything back, so the end state is exactly the three grants below.
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{user}:F"))
        .arg("/grant:r")
        .arg("SYSTEM:F")
        // Administrators by well-known SID: the localized group name varies by display language.
        .arg("/grant:r")
        .arg("*S-1-5-32-544:F")
        // icacls's own success chatter isn't useful; the exit status is what's checked.
        .stdout(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("failed to run icacls: {e}"))?;

    if !status.success() {
        return Err(format!("icacls exited with {status}"));
    }
    Ok(())
}

#[cfg(windows)]
fn current_user_account() -> Result<String, String> {
    let output = std::process::Command::new("whoami")
        .output()
        .map_err(|e| format!("failed to run whoami: {e}"))?;
    if !output.status.success() {
        return Err(format!("whoami exited with {}", output.status));
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        return Err("whoami printed no output".to_string());
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_private_key_pem_creates_parent_dirs_and_writes_the_exact_content() {
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-net-write-key-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("nested").join("client.key");
        let pem = "-----BEGIN PRIVATE KEY-----\nfake\n-----END PRIVATE KEY-----\n";

        write_private_key_pem(&path, pem).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), pem);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A failing permission step leaves an existing key byte-for-byte intact and removes
    /// the temporary file.
    #[test]
    fn a_failed_restrict_step_leaves_an_existing_key_intact() {
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-net-keep-key-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("client.key");
        std::fs::write(&path, "old key").unwrap();

        let err = write_private_key_with(&path, "new key", |_| Err("acl refused".to_string()))
            .unwrap_err();
        assert!(err.contains("acl refused"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old key");
        assert!(!dir.join("client.key.tmp").exists());

        write_private_key_with(&path, "new key", |_| Ok(())).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new key");
        assert!(!dir.join("client.key.tmp").exists());

        std::fs::remove_dir_all(&dir).ok();
    }
}
