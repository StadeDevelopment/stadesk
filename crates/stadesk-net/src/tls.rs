use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use blake2::{Blake2b256, Digest};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use quinn::rustls::crypto::{CryptoProvider, ring, verify_tls13_signature};
use quinn::rustls::pki_types::{
    CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime,
};
use quinn::rustls::{self, DigitallySignedStruct, SignatureScheme};
use quinn::{IdleTimeout, TransportConfig};
use stadesk_core::SERVER_FINGERPRINT_LEN;
use zeroize::Zeroizing;

use crate::error::NetError;

pub const ALPN: &[u8] = b"stadesk/1";
pub const SERVER_NAME: &str = "stadesk";
const CERTIFICATE_FINGERPRINT_DOMAIN: &[u8] = b"stadesk-server-cert-v1\0";
const FINGERPRINT_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const FINGERPRINT_CHARS: usize = (SERVER_FINGERPRINT_LEN * 8).div_ceil(5);
const FINGERPRINT_GROUP: usize = 4;
const KEEP_ALIVE: Duration = Duration::from_secs(5);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ServerFingerprint([u8; SERVER_FINGERPRINT_LEN]);

impl ServerFingerprint {
    pub fn of_certificate(certificate_der: &[u8]) -> Self {
        let mut hasher = Blake2b256::new();
        hasher.update(CERTIFICATE_FINGERPRINT_DOMAIN);
        hasher.update(certificate_der);
        Self(hasher.finalize().into())
    }

    pub fn as_bytes(&self) -> &[u8; SERVER_FINGERPRINT_LEN] {
        &self.0
    }
}

impl fmt::Display for ServerFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut accumulator: u32 = 0;
        let mut bits = 0u32;
        let mut written = 0usize;
        let mut emit = |formatter: &mut fmt::Formatter<'_>, value: u32| -> fmt::Result {
            if written > 0 && written.is_multiple_of(FINGERPRINT_GROUP) {
                formatter.write_str("-")?;
            }
            written += 1;
            let character = FINGERPRINT_ALPHABET[(value & 0x1f) as usize] as char;
            fmt::Write::write_char(formatter, character)
        };
        for byte in self.0 {
            accumulator = (accumulator << 8) | u32::from(byte);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                emit(formatter, accumulator >> bits)?;
            }
            accumulator &= (1 << bits) - 1;
        }
        if bits > 0 {
            emit(formatter, accumulator << (5 - bits))?;
        }
        Ok(())
    }
}

impl fmt::Debug for ServerFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl FromStr for ServerFingerprint {
    type Err = NetError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let mut values = Vec::with_capacity(FINGERPRINT_CHARS);
        for character in input.trim().chars().flat_map(char::to_uppercase) {
            let normalized = match character {
                'O' => '0',
                'I' | 'L' => '1',
                ' ' | '-' | '_' => continue,
                other => other,
            };
            let position = FINGERPRINT_ALPHABET
                .iter()
                .position(|candidate| char::from(*candidate) == normalized)
                .ok_or(NetError::InvalidServerKey)?;
            values.push(position as u32);
        }
        if values.len() != FINGERPRINT_CHARS {
            return Err(NetError::InvalidServerKey);
        }
        let mut out = [0u8; SERVER_FINGERPRINT_LEN];
        let mut accumulator: u32 = 0;
        let mut bits = 0u32;
        let mut index = 0usize;
        for value in values {
            accumulator = (accumulator << 5) | value;
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                out[index] = (accumulator >> bits) as u8;
                index += 1;
                accumulator &= (1 << bits) - 1;
            }
        }
        if accumulator != 0 {
            return Err(NetError::InvalidServerKey);
        }
        Ok(Self(out))
    }
}

pub struct ServerCertificate {
    certificate: CertificateDer<'static>,
    private_key: Zeroizing<Vec<u8>>,
}

impl ServerCertificate {
    pub fn generate() -> Result<Self, NetError> {
        let key = rcgen::KeyPair::generate()?;
        let certificate =
            rcgen::CertificateParams::new(vec![SERVER_NAME.to_owned()])?.self_signed(&key)?;
        Ok(Self {
            certificate: certificate.der().clone(),
            private_key: Zeroizing::new(key.serialize_der()),
        })
    }

    pub fn from_der(certificate_der: Vec<u8>, private_key_pkcs8_der: Vec<u8>) -> Self {
        Self {
            certificate: CertificateDer::from(certificate_der),
            private_key: Zeroizing::new(private_key_pkcs8_der),
        }
    }

    pub fn certificate_der(&self) -> &[u8] {
        self.certificate.as_ref()
    }

    pub fn private_key_der(&self) -> &[u8] {
        &self.private_key
    }

    pub fn fingerprint(&self) -> ServerFingerprint {
        ServerFingerprint::of_certificate(self.certificate.as_ref())
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(ring::default_provider())
}

fn transport() -> Arc<TransportConfig> {
    let mut transport = TransportConfig::default();
    transport.keep_alive_interval(Some(KEEP_ALIVE));
    transport.max_idle_timeout(Some(
        IdleTimeout::try_from(IDLE_TIMEOUT).expect("30 seconds is a valid QUIC idle timeout"),
    ));
    Arc::new(transport)
}

pub fn server_config(certificate: &ServerCertificate) -> Result<quinn::ServerConfig, NetError> {
    let mut tls = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(
            vec![certificate.certificate.clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(certificate.private_key.to_vec())),
        )?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(tls)?));
    config.transport_config(transport());
    Ok(config)
}

fn client_config(verifier: Arc<dyn ServerCertVerifier>) -> Result<quinn::ClientConfig, NetError> {
    let mut tls = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let mut config = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls)?));
    config.transport_config(transport());
    Ok(config)
}

pub fn rendezvous_client_config(
    expected: ServerFingerprint,
) -> Result<quinn::ClientConfig, NetError> {
    client_config(Arc::new(CertificateVerifier {
        expected: Some(expected),
        provider: provider(),
    }))
}

pub fn peer_client_config() -> Result<quinn::ClientConfig, NetError> {
    client_config(Arc::new(CertificateVerifier {
        expected: None,
        provider: provider(),
    }))
}

#[derive(Debug)]
struct CertificateVerifier {
    expected: Option<ServerFingerprint>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for CertificateVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self.expected {
            Some(expected)
                if ServerFingerprint::of_certificate(end_entity.as_ref()) != expected =>
            {
                Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::ApplicationVerificationFailure,
                ))
            }
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General(
            "TLS 1.2 is not supported".to_owned(),
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_text_round_trips_and_has_grouped_shape() {
        let certificate = ServerCertificate::generate().unwrap();
        let fingerprint = certificate.fingerprint();
        let text = fingerprint.to_string();
        assert_eq!(text.replace('-', "").len(), FINGERPRINT_CHARS);
        assert_eq!(
            text.split('-').count(),
            FINGERPRINT_CHARS.div_ceil(FINGERPRINT_GROUP)
        );
        assert_eq!(text.parse::<ServerFingerprint>().unwrap(), fingerprint);
        assert_eq!(
            text.to_lowercase()
                .replace('-', " ")
                .parse::<ServerFingerprint>()
                .unwrap(),
            fingerprint
        );
    }

    #[test]
    fn fingerprint_of_all_ones_uses_every_bit() {
        let fingerprint = ServerFingerprint([0xff; SERVER_FINGERPRINT_LEN]);
        let text = fingerprint.to_string().replace('-', "");
        assert!(text[..FINGERPRINT_CHARS - 1].chars().all(|c| c == 'Z'));
        assert!(text.ends_with('G'));
        assert_eq!(text.parse::<ServerFingerprint>().unwrap(), fingerprint);
    }

    #[test]
    fn fingerprint_parse_rejects_bad_length_characters_and_padding_bits() {
        let text = ServerFingerprint([0; SERVER_FINGERPRINT_LEN]).to_string();
        assert!(text[..text.len() - 1].parse::<ServerFingerprint>().is_err());
        assert!(format!("{text}0").parse::<ServerFingerprint>().is_err());
        assert!(text.replace('0', "U").parse::<ServerFingerprint>().is_err());
        let mut padded = text.replace('-', "");
        padded.pop();
        padded.push('1');
        assert!(padded.parse::<ServerFingerprint>().is_err());
    }

    #[test]
    fn different_certificates_have_different_fingerprints() {
        let first = ServerCertificate::generate().unwrap();
        let second = ServerCertificate::generate().unwrap();
        assert_ne!(first.fingerprint(), second.fingerprint());
        let restored = ServerCertificate::from_der(
            first.certificate_der().to_vec(),
            first.private_key_der().to_vec(),
        );
        assert_eq!(restored.fingerprint(), first.fingerprint());
    }
}
