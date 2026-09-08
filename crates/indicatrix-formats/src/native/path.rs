//! `.asc` <-> native-sidecar path arithmetic -- no filesystem access, see the parent
//! module's doc comment ("Extension and layout") for the naming convention.

/// The native sidecar extension used for new saves, minus the leading dot.
///
/// A compound extension (`set_extension` replaces only the text after the LAST `.`,
/// so `"foo.asc"` -> `"foo.indicatrix.toml"` in one call -- see
/// [`native_path_for_asc`]), not a bare `.indicatrix`, so the file is self-evidently
/// text before anything else is known about it.
pub const NATIVE_EXTENSION_SUFFIX: &str = "indicatrix.toml";

/// The extension this format was written under before it was renamed from `gemcut`.
/// Files saved under this suffix still load; see [`asc_path_for_native`].
pub const LEGACY_NATIVE_EXTENSION_SUFFIX: &str = "gemcut.toml";

/// The native sidecar path for a given `.asc` path: `foo.asc` -> `foo.indicatrix.toml`,
/// same directory.
///
/// Pure path arithmetic, no filesystem access -- `apps/indicatrix-cut` uses this to
/// default a Save dialog's suggested native file name next to a chosen `.asc` path.
#[must_use]
pub fn native_path_for_asc(asc_path: &std::path::Path) -> std::path::PathBuf {
    let mut native = asc_path.to_path_buf();
    native.set_extension(NATIVE_EXTENSION_SUFFIX);
    native
}

/// The reverse of [`native_path_for_asc`]: strips a current or legacy native suffix
/// and appends `.asc`.
///
/// `None` when `native_path`'s file name ends with neither suffix, or has no file
/// name at all.
///
/// This is only ever a NAMING guess -- [`crate::native::NativeDesignFile::asc_filename`]
/// is the authoritative pointer to the real paired file once a native file has
/// actually been parsed (see that field's own doc comment); this function exists for
/// the moment BEFORE that, when a caller has only a native file's own path (e.g. an
/// "Open" dialog defaulting the initial directory for its own `.asc` picker).
#[must_use]
pub fn asc_path_for_native(native_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let file_name = native_path.file_name()?.to_str()?;
    let stem = file_name
        .strip_suffix(&format!(".{NATIVE_EXTENSION_SUFFIX}"))
        .or_else(|| file_name.strip_suffix(&format!(".{LEGACY_NATIVE_EXTENSION_SUFFIX}")))?;
    Some(native_path.with_file_name(format!("{stem}.asc")))
}
