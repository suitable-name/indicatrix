//! Authenticated setup boundary. A connection test never uploads a scene.
use crate::{
    platform::Platform,
    settings::{MobileSettings, RenderBackend},
};
use indicatrix_net::{
    client,
    messages::{Backend, RenderCapability},
    tls,
};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

pub mod final_image;
pub mod jobs;
pub mod rendering;

const MAX_BUNDLE_BYTES: usize = 128 * 1024;

/// Stored only through the platform credential vault, never regular settings/export.
#[derive(Serialize, Deserialize)]
struct Credentials {
    ca: String,
    certificate: String,
    key: String,
}
impl Drop for Credentials {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

#[derive(Debug, Clone)]
pub struct ConnectionReport {
    pub certificate_identity: String,
    pub render: Option<RenderCapability>,
    pub library: bool,
    pub zoning: bool,
}
impl ConnectionReport {
    #[must_use]
    pub fn summary(&self) -> String {
        match &self.render {
            Some(capability) => format!(
                "Authenticated, compatible {:?} endpoint · up to {} pixels · cadence ≥ {} ms · HDR {}. {}",
                capability.backend,
                capability.max_pixels,
                capability.min_cadence_ms,
                if capability.hdr {
                    "advertised"
                } else {
                    "unsupported"
                },
                if matches!(capability.backend, Backend::Coordinator { .. }) {
                    "DisplayOnly is supported by coordinators."
                } else {
                    "Standalone workers require FullData."
                }
            ),
            None if self.library => {
                "Authenticated library-only endpoint; no render capacity.".into()
            }
            None => "Authenticated endpoint currently advertises no render capacity.".into(),
        }
    }
}

/// Validated, still-uninstalled credentials. Dropping an obsolete claim erases its key.
pub struct Enrollment {
    encoded: Zeroizing<String>,
}

impl Enrollment {
    /// Commit only after the UI accepts the still-current request. Once accepted,
    /// this short atomic vault write finishes on its worker even if the page closes.
    ///
    /// # Errors
    ///
    /// The platform's message when the vault write fails.
    pub fn install(self, platform: &dyn Platform) -> Result<(), String> {
        platform
            .store_remote_credentials(&self.encoded)
            .map_err(|error| error.to_string())
    }
}

/// Redeems and validates the CA-pinned enrollment protocol without changing the vault.
///
/// The caller must accept the current request before installing the returned bundle.
///
/// # Errors
///
/// A message when the enrollment is cancelled or interrupted, the token cannot be
/// redeemed, or the credential bundle is invalid or too large.
pub fn enroll(
    token: &str,
    enrollment_address: &str,
    cancelled: &AtomicBool,
) -> Result<Enrollment, String> {
    if cancelled.load(Ordering::Acquire) {
        return Err("Enrollment cancelled.".into());
    }
    let bundle = indicatrix_net::enroll::claim(token, enrollment_address)
        .map_err(|error| error.to_string())?;
    if cancelled.load(Ordering::Acquire) {
        return Err(
            "Enrollment interrupted; credentials were not installed. Redeem a new token if needed."
                .into(),
        );
    }
    let credentials = Credentials {
        ca: bundle.ca_pem,
        certificate: bundle.client_cert_pem,
        key: bundle.client_key_pem.to_string(),
    };
    configuration(&credentials)?;
    let encoded =
        Zeroizing::new(serde_json::to_string(&credentials).map_err(|error| error.to_string())?);
    if encoded.len() > MAX_BUNDLE_BYTES {
        return Err("Credential bundle exceeds the phone limit.".into());
    }
    Ok(Enrollment { encoded })
}

/// Erases the pairing credentials: the encrypted bundle and the platform key that wraps it.
///
/// Having nothing stored is a success, and the call can be repeated. Afterwards
/// [`test_connection`] and every other connection say "Enroll this phone first." until a
/// new token is redeemed.
///
/// This does not touch the settings; call [`clear_server_settings`] once it has
/// succeeded. It also cannot revoke the phone's certificate on the server: the server
/// keeps its own list of paired phones and its server jobs until its operator removes
/// them.
///
/// # Errors
///
/// The platform's message when the bundle or its key could not be erased. The settings
/// then still say the phone is enrolled, so the user can try again.
pub fn forget(platform: &dyn Platform) -> Result<(), String> {
    platform
        .forget_remote_credentials()
        .map_err(|error| error.to_string())
}

/// Clears what the settings remember about the paired server: its name, address and enrolled flag.
///
/// A selected Remote render backend goes back to Automatic, so no
/// render or export tries to reach a server that was forgotten. The transfer mode, the
/// update interval and the metered-network choice are preferences, not part of the
/// server's identity, and stay.
///
/// Returns whether the render backend was switched away from Remote.
pub fn clear_server_settings(settings: &mut MobileSettings) -> bool {
    settings.remote.name.clear();
    settings.remote.address.clear();
    settings.remote.enrolled = false;
    let was_remote = settings.render.backend == RenderBackend::Remote;
    if was_remote {
        settings.render.backend = RenderBackend::Automatic;
    }
    was_remote
}

fn configuration(credentials: &Credentials) -> Result<Arc<rustls::ClientConfig>, String> {
    let ca = rustls_pemfile::certs(&mut credentials.ca.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    if ca.is_empty() {
        return Err("The credential bundle contains no CA certificate.".into());
    }
    let mut roots = rustls::RootCertStore::empty();
    for certificate in ca {
        roots.add(certificate).map_err(|error| error.to_string())?;
    }
    let certificates = rustls_pemfile::certs(&mut credentials.certificate.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let key = rustls_pemfile::private_key(&mut credentials.key.as_bytes())
        .map_err(|error| error.to_string())?
        .ok_or("No TLS private key in the credential bundle.")?;
    tls::client_config(roots, certificates, key).map_err(|error| error.to_string())
}

/// Connects with the stored credentials and reports the endpoint's identity.
///
/// One worker owns both TLS directions throughout connect/handshake. Finite socket
/// polls preserve partial protocol reads while cancellation and deadlines are checked.
///
/// # Errors
///
/// A message when no credentials are enrolled, the connection or handshake fails or is
/// cancelled, or the endpoint supplies no identity.
pub fn test_connection(
    platform: &dyn Platform,
    address: &str,
    cancelled: &AtomicBool,
) -> Result<ConnectionReport, String> {
    let cancelled = || cancelled.load(Ordering::Acquire);
    let (stream, welcome) = connect(platform, address, &cancelled)?;
    let certificate = stream
        .tls
        .conn
        .peer_certificates()
        .and_then(|chain| chain.first())
        .ok_or("The TLS endpoint supplied no identity.")?;
    let certificate_identity = tls::fingerprint_to_hex(&tls::fingerprint(certificate));
    Ok(ConnectionReport {
        certificate_identity,
        render: welcome.render,
        library: welcome.library,
        zoning: welcome.zoning,
    })
}

fn connect<'a>(
    platform: &dyn Platform,
    address: &str,
    cancelled: &'a dyn Fn() -> bool,
) -> Result<(DeadlineStream<'a>, indicatrix_net::messages::Welcome), String> {
    let encoded = Zeroizing::new(
        platform
            .load_remote_credentials()
            .map_err(|error| error.to_string())?
            .ok_or("Enroll this phone first.")?,
    );
    if encoded.len() > MAX_BUNDLE_BYTES {
        return Err("Credential bundle exceeds the phone limit.".into());
    }
    let credentials: Credentials = serde_json::from_str(&encoded)
        .map_err(|_| "The stored credential bundle is invalid.".to_owned())?;
    let config = configuration(&credentials)?;
    let host = tls::host_for_server_name(address).ok_or("Enter a host:port rendering address.")?;
    let server_name = rustls::pki_types::ServerName::try_from(host.to_owned())
        .map_err(|error| error.to_string())?;
    let addresses = address
        .to_socket_addrs()
        .map_err(|error| error.to_string())?;
    let mut connected = None;
    let mut last = "No network address was found.".to_owned();
    for endpoint in addresses.take(4) {
        if cancelled() {
            return Err("Connection cancelled.".into());
        }
        match TcpStream::connect_timeout(&endpoint, Duration::from_secs(2)) {
            Ok(socket) => {
                connected = Some(socket);
                break;
            }
            Err(error) => last = error.to_string(),
        }
    }
    let socket = connected.ok_or(last)?;
    socket
        .set_nodelay(true)
        .map_err(|error| error.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|error| error.to_string())?;
    socket
        .set_write_timeout(Some(Duration::from_millis(200)))
        .map_err(|error| error.to_string())?;
    let connection =
        rustls::ClientConnection::new(config, server_name).map_err(|error| error.to_string())?;
    let mut stream = DeadlineStream {
        tls: rustls::StreamOwned::new(connection, socket),
        cancelled,
        deadline: Instant::now() + Duration::from_secs(12),
    };
    // The shared helper verifies protocol, engine build and source identity. Merely
    // establishing a TLS socket never grants render compatibility.
    let welcome = client::handshake::handshake(&mut stream).map_err(|error| error.to_string())?;
    Ok((stream, welcome))
}

struct DeadlineStream<'a> {
    tls: rustls::StreamOwned<rustls::ClientConnection, TcpStream>,
    cancelled: &'a dyn Fn() -> bool,
    deadline: Instant,
}
impl DeadlineStream<'_> {
    fn check(&self) -> io::Result<()> {
        if (self.cancelled)() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Connection cancelled",
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Connection deadline exceeded",
            ));
        }
        Ok(())
    }
}
fn retryable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}
impl Read for DeadlineStream<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            self.check()?;
            match self.tls.read(bytes) {
                Err(error) if retryable(&error) => {}
                result => return result,
            }
        }
    }
}
impl Write for DeadlineStream<'_> {
    /// A write retries through the 200 ms socket timeouts until the TLS layer takes the
    /// bytes, so a large upload (an HDR asset is tens of MiB) survives a full send buffer.
    ///
    /// Retrying is safe because of how `rustls::StreamOwned::write` is built: it completes
    /// any earlier pending output first (a timeout there is returned BEFORE any of `bytes`
    /// is consumed), then buffers `bytes` and attempts the socket write while ignoring its
    /// errors (the already-consumed bytes are reported as written). A timeout error
    /// therefore always means "none of these bytes were taken", never "some were". Any
    /// other error, or the deadline or cancellation, still ends the connection.
    ///
    /// The deadline is a stall limit for writes: every accepted write pushes it out to
    /// [`WRITE_STALL`] from now, so a slow but moving upload is not cut off.
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            self.check()?;
            match self.tls.write(bytes) {
                Err(error) if retryable(&error) => {}
                Ok(written) => {
                    self.deadline = self.deadline.max(Instant::now() + WRITE_STALL);
                    return Ok(written);
                }
                result => return result,
            }
        }
    }
    /// Flushing retries like [`Self::write`]: rustls keeps the unsent records buffered, so
    /// a timeout leaves nothing duplicated.
    fn flush(&mut self) -> io::Result<()> {
        loop {
            self.check()?;
            match self.tls.flush() {
                Err(error) if retryable(&error) => {}
                result => return result,
            }
        }
    }
}

/// How long a write may make no progress before the connection is given up.
const WRITE_STALL: Duration = Duration::from_secs(12);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        platform::bare::BarePlatform, platform_mock::MockPlatform, settings::RemoteTransfer,
    };

    #[test]
    fn missing_render_capacity_is_never_render_ready() {
        let report = ConnectionReport {
            certificate_identity: String::new(),
            render: None,
            library: true,
            zoning: false,
        };
        assert!(report.summary().contains("library-only"));
        assert!(report.summary().contains("no render capacity"));
    }
    #[test]
    fn empty_credentials_do_not_disable_tls_verification() {
        let credentials = Credentials {
            ca: String::new(),
            certificate: String::new(),
            key: String::new(),
        };
        assert!(configuration(&credentials).is_err());
    }

    fn paired_platform() -> MockPlatform {
        let platform = MockPlatform::default();
        Enrollment {
            encoded: Zeroizing::new("bundle".to_owned()),
        }
        .install(&platform)
        .expect("the mock vault accepts the bundle");
        assert_eq!(platform.stored_credentials().as_deref(), Some("bundle"));
        platform
    }

    #[test]
    fn forgetting_erases_the_stored_bundle() {
        let platform = paired_platform();
        forget(&platform).expect("the mock vault erases");
        assert_eq!(platform.stored_credentials(), None);
        assert_eq!(platform.load_remote_credentials(), Ok(None));
    }

    #[test]
    fn a_forgotten_server_cannot_be_reached_until_the_phone_pairs_again() {
        let platform = paired_platform();
        forget(&platform).expect("the mock vault erases");
        let error = test_connection(&platform, "127.0.0.1:1", &AtomicBool::new(false))
            .expect_err("without credentials nothing connects");
        assert_eq!(error, "Enroll this phone first.");
    }

    #[test]
    fn forgetting_with_nothing_stored_succeeds_and_can_be_repeated() {
        let platform = MockPlatform::default();
        assert_eq!(forget(&platform), Ok(()));
        assert_eq!(forget(&platform), Ok(()));
        assert_eq!(platform.stored_credentials(), None);
    }

    #[test]
    fn a_failed_erase_reports_the_error_and_a_retry_finishes_it() {
        let platform = paired_platform();
        platform.set_forget_failure(true);
        let error = forget(&platform).expect_err("the vault refuses");
        assert!(error.contains("could not be erased"), "{error}");
        assert_eq!(platform.stored_credentials().as_deref(), Some("bundle"));

        platform.set_forget_failure(false);
        assert_eq!(forget(&platform), Ok(()));
        assert_eq!(platform.stored_credentials(), None);
    }

    #[test]
    fn a_platform_without_a_vault_cannot_claim_to_have_forgotten() {
        // The trait's own default (nothing overridden), not a mock's copy of its message.
        let platform = BarePlatform::new();
        let error = forget(&platform).expect_err("no vault, no claim");
        assert!(error.contains("no encrypted credential vault"), "{error}");

        // A mock that is told it has no vault behaves the same.
        let mock = MockPlatform::default();
        mock.set_vault_available(false);
        let error = forget(&mock).expect_err("no vault, no claim");
        assert!(error.contains("no encrypted credential vault"), "{error}");
    }

    #[test]
    fn clearing_the_server_settings_keeps_the_preferences() {
        let mut settings = MobileSettings::default();
        settings.remote.name = "Studio".to_owned();
        settings.remote.address = "192.168.1.5:7000".to_owned();
        settings.remote.enrolled = true;
        settings.remote.transfer = RemoteTransfer::FullData;
        settings.remote.cadence_ms = 900;
        settings.remote.allow_metered = true;
        settings.render.backend = RenderBackend::Remote;

        assert!(clear_server_settings(&mut settings));

        assert_eq!(settings.remote.name, "");
        assert_eq!(settings.remote.address, "");
        assert!(!settings.remote.enrolled);
        assert_eq!(settings.render.backend, RenderBackend::Automatic);
        assert_eq!(settings.remote.transfer, RemoteTransfer::FullData);
        assert_eq!(settings.remote.cadence_ms, 900);
        assert!(settings.remote.allow_metered);
    }

    #[test]
    fn clearing_the_server_settings_leaves_another_backend_alone() {
        for backend in [
            RenderBackend::Automatic,
            RenderBackend::DeviceCpu,
            RenderBackend::DeviceGpu,
        ] {
            let mut settings = MobileSettings::default();
            settings.remote.address = "host:1".to_owned();
            settings.remote.enrolled = true;
            settings.render.backend = backend;

            assert!(!clear_server_settings(&mut settings));

            assert_eq!(settings.render.backend, backend);
            assert_eq!(settings.remote.address, "");
            assert!(!settings.remote.enrolled);
        }
    }
}
