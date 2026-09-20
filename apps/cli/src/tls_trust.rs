//! Classification of TLS certificate-verification failures into actionable
//! categories.
//!
//! ureq 3.x surfaces a failed rustls handshake as `ureq::Error::Io` — the
//! verification error is wrapped in an `io::Error` by the rustls stream, NOT
//! delivered as `ureq::Error::Rustls` (that variant only carries
//! configuration-time errors). The classifier therefore walks the io error's
//! source chain and downcasts to [`rustls::Error`]. A unit test pins this
//! wrapping behavior since it depends on rustls internals.

use rustls::CertificateError;

/// A certificate-verification failure the user can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsTrustFailure {
    /// The server certificate does not chain to any trusted root
    /// (self-signed / private-CA deployment without a configured extra CA,
    /// or a mismatching CA).
    UntrustedIssuer,
    /// The certificate's SAN does not cover the requested host.
    NameMismatch,
    /// The certificate has expired.
    Expired,
    /// The certificate is not yet valid (clock skew or premature rollout).
    NotYetValid,
    /// The certificate is not fit for TLS server authentication (for example
    /// a CA certificate presented as the end-entity certificate).
    WrongPurpose,
}

impl TlsTrustFailure {
    /// One-line English hint for API-facing error envelopes.
    #[must_use]
    pub fn hint(self) -> &'static str {
        match self {
            Self::UntrustedIssuer => {
                "server certificate is not trusted (configure egress extra_ca_file for private-CA deployments)"
            }
            Self::NameMismatch => "server certificate does not match the requested host",
            Self::Expired => "server certificate has expired",
            Self::NotYetValid => "server certificate is not yet valid",
            Self::WrongPurpose => {
                "server certificate is not valid for TLS server authentication (possibly a CA certificate used as the server certificate)"
            }
        }
    }
}

/// Classify a transport error as a certificate-verification failure, or
/// `None` for everything else (timeouts, refused connections, protocol
/// errors, ...).
#[must_use]
pub fn classify_tls_failure(error: &ureq::Error) -> Option<TlsTrustFailure> {
    let rustls_error = match error {
        ureq::Error::Io(io) => find_rustls_error(io)?,
        ureq::Error::Rustls(error) => error,
        _ => return None,
    };
    let rustls::Error::InvalidCertificate(certificate_error) = rustls_error else {
        return None;
    };
    Some(match certificate_error {
        CertificateError::UnknownIssuer => TlsTrustFailure::UntrustedIssuer,
        CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => {
            TlsTrustFailure::NameMismatch
        }
        CertificateError::Expired | CertificateError::ExpiredContext { .. } => {
            TlsTrustFailure::Expired
        }
        CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => {
            TlsTrustFailure::NotYetValid
        }
        CertificateError::InvalidPurpose | CertificateError::InvalidPurposeContext { .. } => {
            TlsTrustFailure::WrongPurpose
        }
        _ => return None,
    })
}

fn find_rustls_error(io: &std::io::Error) -> Option<&rustls::Error> {
    let mut source: Option<&(dyn std::error::Error + 'static)> =
        io.get_ref().map(|error| error as _);
    while let Some(error) = source {
        if let Some(rustls_error) = error.downcast_ref::<rustls::Error>() {
            return Some(rustls_error);
        }
        source = error.source();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{TlsTrustFailure, classify_tls_failure};
    use rustls::CertificateError;

    fn io_wrapped(certificate_error: CertificateError) -> ureq::Error {
        // The exact wrapping rustls' stream applies to a failed handshake.
        let rustls_error = rustls::Error::InvalidCertificate(certificate_error);
        ureq::Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            rustls_error,
        ))
    }

    #[test]
    fn certificate_failures_classify_from_io_wrapped_rustls_errors() {
        let cases = [
            (
                CertificateError::UnknownIssuer,
                TlsTrustFailure::UntrustedIssuer,
            ),
            (
                CertificateError::NotValidForName,
                TlsTrustFailure::NameMismatch,
            ),
            (CertificateError::Expired, TlsTrustFailure::Expired),
            (CertificateError::NotValidYet, TlsTrustFailure::NotYetValid),
            (
                CertificateError::InvalidPurpose,
                TlsTrustFailure::WrongPurpose,
            ),
        ];
        for (certificate_error, expected) in cases {
            assert_eq!(
                classify_tls_failure(&io_wrapped(certificate_error)),
                Some(expected)
            );
        }
    }

    #[test]
    fn non_certificate_errors_stay_unclassified() {
        let refused = ureq::Error::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        ));
        assert_eq!(classify_tls_failure(&refused), None);
        let alert = ureq::Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            rustls::Error::AlertReceived(rustls::AlertDescription::HandshakeFailure),
        ));
        assert_eq!(classify_tls_failure(&alert), None);
    }
}
