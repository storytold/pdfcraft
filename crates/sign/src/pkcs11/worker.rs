//! Private, versioned one-request protocol. PIN bytes are framed separately from JSON.
use super::{backend, process};
use crate::{Certificate, DigestAlg, SignError};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use zeroize::Zeroizing;

pub(super) const ARGUMENT: &str = "--pdfcraft-pkcs11-worker-v1";
pub(super) const LIMIT: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    pub certificate: Vec<u8>,
    pub chain: Vec<Vec<u8>>,
    pub label: Option<String>,
    pub serial: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum Request {
    List { decryption: bool },
    Sign { identity: Identity, algorithm: String, message: Vec<u8> },
    Decrypt { identity: Identity, ciphertext: Vec<u8> },
}

/// Dispatch the private worker mode before logging, UI, or normal CLI initialization.
/// The mode accepts exactly one bounded request on stdin and exits after replying. It
/// opens no listener, writes no file, and never reads a PIN from its environment.
pub fn worker_if_requested() -> Option<std::process::ExitCode> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(ARGUMENT)) {
        return None;
    }
    if args.next().is_some() {
        return Some(std::process::ExitCode::FAILURE);
    }
    let result = run();
    Some(if result.is_ok() { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE })
}

fn read_frame(reader: &mut impl Read, limit: usize) -> Result<Zeroizing<Vec<u8>>, SignError> {
    let mut length = [0; 4];
    reader.read_exact(&mut length).map_err(process::io_error)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > limit {
        return Err(SignError::Malformed("smartcard worker request exceeds its limit".into()));
    }
    let mut bytes = Zeroizing::new(vec![0; length]);
    reader.read_exact(&mut bytes).map_err(process::io_error)?;
    Ok(bytes)
}
fn run() -> Result<(), SignError> {
    // Also stop if the UI exits before its cancellation thread can reap this helper.
    rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::KILL)).map_err(|e| process::io_error(e.into()))?;
    if let Some(expected) = std::env::var_os("PDFCRAFT_PKCS11_PARENT") {
        let expected = expected
            .to_str()
            .and_then(|p| p.parse::<i32>().ok())
            .filter(|p| *p > 0)
            .ok_or_else(|| SignError::Malformed("invalid smartcard worker parent identity".into()))?;
        if Some(expected) != rustix::process::getppid().map(|p| p.as_raw_pid()) {
            return Err(SignError::Crypto("the smartcard worker's parent exited".into()));
        }
    }
    // A driver crash must not persist a PIN or a decrypted content key in a core dump.
    rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable).map_err(|e| process::io_error(e.into()))?;
    let mut input = std::io::stdin().lock();
    let request = read_frame(&mut input, LIMIT)?;
    let request: Request = serde_json::from_slice(&request).map_err(|_| SignError::Malformed("invalid smartcard worker request".into()))?;
    let pin = read_frame(&mut input, 256)?;
    let pin = std::str::from_utf8(&pin).map_err(|_| SignError::Malformed("the token PIN must be UTF-8".into()))?;
    let result = execute(request, (!pin.is_empty()).then_some(pin));
    let (status, bytes) = match result {
        Ok(bytes) => (0, bytes),
        Err(SignError::TokenChanged) => (2, Zeroizing::new(Vec::new())),
        Err(error) => (1, Zeroizing::new(error.to_string().into_bytes())),
    };
    if bytes.len() > LIMIT {
        return Err(SignError::Crypto("smartcard worker response exceeds its limit".into()));
    }
    let mut output = std::io::stdout().lock();
    output.write_all(&[status]).map_err(process::io_error)?;
    output.write_all(&(bytes.len() as u32).to_be_bytes()).map_err(process::io_error)?;
    output.write_all(&bytes).map_err(process::io_error)?;
    output.flush().map_err(process::io_error)
}
fn execute(request: Request, pin: Option<&str>) -> Result<Zeroizing<Vec<u8>>, SignError> {
    match request {
        Request::List { decryption } => {
            let identities: Vec<Identity> = if decryption {
                backend::decryption_identities()?
                    .into_iter()
                    .map(|id| Identity {
                        certificate: id.certificate().raw.clone(),
                        chain: Vec::new(),
                        label: Some(id.token_label().into()),
                        serial: id.token_serial().into(),
                    })
                    .collect()
            } else {
                backend::list()?
                    .into_iter()
                    .map(|(id, serial)| Identity {
                        certificate: id.certificate.raw,
                        chain: id.chain.into_iter().map(|c| c.raw).collect(),
                        label: id.friendly_name,
                        serial,
                    })
                    .collect()
            };
            serde_json::to_vec(&identities).map(Zeroizing::new).map_err(|_| SignError::Crypto("could not encode smartcard identities".into()))
        }
        Request::Sign { identity, algorithm, message } => {
            let alg = DigestAlg::from_oid(&algorithm).ok_or_else(|| SignError::Unsupported("unknown smartcard digest".into()))?;
            // Select the same public certificate and physical token before authenticating.
            let reference = backend::reference(&Certificate::parse(&identity.certificate)?);
            let id = backend::find_with_pin(&reference, pin, &identity.serial)?;
            if id.certificate.raw != identity.certificate {
                return Err(SignError::TokenChanged);
            }
            id.key.sign(alg, &message).map(Zeroizing::new)
        }
        Request::Decrypt { identity, ciphertext } => {
            let mut matching = backend::decryption_identities()?
                .into_iter()
                .filter(|id| id.certificate().raw == identity.certificate && id.token_serial() == identity.serial);
            let id = matching.next().ok_or_else(|| SignError::Crypto("the selected card was removed or replaced; refresh cards".into()))?;
            if matching.next().is_some() {
                return Err(SignError::Crypto("the selected encryption identity is ambiguous".into()));
            }
            id.decrypt_key(&ciphertext, pin)
        }
    }
}
