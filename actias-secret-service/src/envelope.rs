//! Envelope encryption for secret values: a fresh data key per secret
//! version encrypts exactly one value, and the labeled master key (KEK)
//! only ever wraps data keys. Master rotation re-wraps rows without
//! touching ciphertexts; a KMS integration would replace only the wrap and
//! unwrap calls here.

use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead};
use std::collections::HashMap;
use zeroize::Zeroizing;

/// Bytes of an AES-256-GCM key, master and data keys alike.
pub const KEY_LEN: usize = 32;
/// Bytes of a GCM nonce; prefixed onto wrapped data keys, stored as its
/// own column for value ciphertexts.
pub const NONCE_LEN: usize = 12;

/// Why an operation failed; carried to logs, never to callers, and never
/// holding key or value material.
#[derive(Debug, PartialEq, Eq)]
pub enum CryptoError {
    /// The row was wrapped by a master this process does not hold.
    UnknownKek(String),
    /// Authentication failed or the stored shape is wrong.
    Corrupt,
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CryptoError::UnknownKek(id) => write!(f, "unknown kek '{id}'"),
            CryptoError::Corrupt => write!(f, "stored secret could not be opened"),
        }
    }
}

/// A value encrypted under its own data key: exactly one row's worth.
pub struct Sealed {
    pub kek_id: String,
    pub dek_wrapped: Vec<u8>,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

/// The process's master keys: one active for every new wrap, any number
/// (in practice one) of previous masters kept readable during rotation.
pub struct Envelope {
    active: String,
    keks: HashMap<String, Zeroizing<[u8; KEY_LEN]>>,
}

/// Fills a buffer from the operating system's entropy source.
fn random(buf: &mut [u8]) -> Result<(), CryptoError> {
    getrandom::fill(buf).map_err(|_| CryptoError::Corrupt)
}

fn cipher(key: &[u8; KEY_LEN]) -> Result<Aes256Gcm, CryptoError> {
    Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::Corrupt)
}

impl Envelope {
    pub fn new(
        active_id: String,
        active_key: Zeroizing<[u8; KEY_LEN]>,
        previous: Option<(String, Zeroizing<[u8; KEY_LEN]>)>,
    ) -> Self {
        let mut keks = HashMap::new();
        keks.insert(active_id.clone(), active_key);
        if let Some((id, key)) = previous {
            keks.insert(id, key);
        }
        Envelope {
            active: active_id,
            keks,
        }
    }

    fn kek(&self, id: &str) -> Result<&Zeroizing<[u8; KEY_LEN]>, CryptoError> {
        self.keks
            .get(id)
            .ok_or_else(|| CryptoError::UnknownKek(id.to_owned()))
    }

    /// Encrypts one value under a fresh data key wrapped by the active
    /// master.
    ///
    /// # Errors
    /// Returns [`CryptoError::Corrupt`] when the aead refuses to seal.
    pub fn seal(&self, plaintext: &[u8]) -> Result<Sealed, CryptoError> {
        let mut dek = Zeroizing::new([0u8; KEY_LEN]);
        random(dek.as_mut())?;
        let mut nonce = [0u8; NONCE_LEN];
        random(&mut nonce)?;

        let ciphertext = cipher(&dek)?
            .encrypt(&nonce.into(), plaintext)
            .map_err(|_| CryptoError::Corrupt)?;

        Ok(Sealed {
            kek_id: self.active.clone(),
            dek_wrapped: self.wrap(&dek)?,
            nonce: nonce.to_vec(),
            ciphertext,
        })
    }

    /// Decrypts one stored value.
    pub fn open(
        &self,
        kek_id: &str,
        dek_wrapped: &[u8],
        nonce: &[u8],
        ciphertext: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        let dek = self.unwrap(kek_id, dek_wrapped)?;
        let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| CryptoError::Corrupt)?;

        cipher(&dek)?
            .decrypt(&nonce.into(), ciphertext)
            .map(Zeroizing::new)
            .map_err(|_| CryptoError::Corrupt)
    }

    /// Re-wraps a row's data key under the active master; the value
    /// ciphertext is untouched. The master-rotation sweep, one row at a
    /// time; the sweep command arrives with the first real rotation, and
    /// tests keep the seam honest until then.
    #[allow(dead_code)]
    pub fn rewrap(
        &self,
        kek_id: &str,
        dek_wrapped: &[u8],
    ) -> Result<(String, Vec<u8>), CryptoError> {
        let dek = self.unwrap(kek_id, dek_wrapped)?;
        Ok((self.active.clone(), self.wrap(&dek)?))
    }

    /// Wraps a data key under the active master, wrap nonce prefixed.
    fn wrap(&self, dek: &Zeroizing<[u8; KEY_LEN]>) -> Result<Vec<u8>, CryptoError> {
        let mut wrap_nonce = [0u8; NONCE_LEN];
        random(&mut wrap_nonce)?;

        let wrapped = cipher(self.kek(&self.active)?)?
            .encrypt(&wrap_nonce.into(), dek.as_slice())
            .map_err(|_| CryptoError::Corrupt)?;

        let mut out = wrap_nonce.to_vec();
        out.extend_from_slice(&wrapped);
        Ok(out)
    }

    fn unwrap(
        &self,
        kek_id: &str,
        dek_wrapped: &[u8],
    ) -> Result<Zeroizing<[u8; KEY_LEN]>, CryptoError> {
        if dek_wrapped.len() <= NONCE_LEN {
            return Err(CryptoError::Corrupt);
        }
        let wrap_nonce: [u8; NONCE_LEN] = dek_wrapped[..NONCE_LEN]
            .try_into()
            .map_err(|_| CryptoError::Corrupt)?;

        let dek = cipher(self.kek(kek_id)?)?
            .decrypt(&wrap_nonce.into(), &dek_wrapped[NONCE_LEN..])
            .map_err(|_| CryptoError::Corrupt)?;

        let dek: [u8; KEY_LEN] = dek.try_into().map_err(|_| CryptoError::Corrupt)?;
        Ok(Zeroizing::new(dek))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope() -> Envelope {
        Envelope::new("kek-a".to_owned(), Zeroizing::new([7u8; KEY_LEN]), None)
    }

    #[test]
    fn a_sealed_value_opens_to_the_plaintext() {
        let envelope = envelope();
        let sealed = envelope.seal(b"sk_live_51H8xQ2").expect("seals");

        let opened = envelope
            .open(
                &sealed.kek_id,
                &sealed.dek_wrapped,
                &sealed.nonce,
                &sealed.ciphertext,
            )
            .expect("opens");
        assert_eq!(opened.as_slice(), b"sk_live_51H8xQ2");
    }

    #[test]
    fn tampering_with_any_part_refuses_to_open() {
        let envelope = envelope();
        let sealed = envelope.seal(b"value").expect("seals");

        let mut ciphertext = sealed.ciphertext.clone();
        ciphertext[0] ^= 1;
        assert_eq!(
            envelope.open(
                &sealed.kek_id,
                &sealed.dek_wrapped,
                &sealed.nonce,
                &ciphertext
            ),
            Err(CryptoError::Corrupt),
        );

        let mut wrapped = sealed.dek_wrapped.clone();
        let last = wrapped.len() - 1;
        wrapped[last] ^= 1;
        assert_eq!(
            envelope.open(&sealed.kek_id, &wrapped, &sealed.nonce, &sealed.ciphertext),
            Err(CryptoError::Corrupt),
        );
    }

    #[test]
    fn a_master_rotation_rewraps_without_touching_the_ciphertext() {
        let old = envelope();
        let sealed = old.seal(b"survives rotation").expect("seals");

        // The new process holds kek-b active with kek-a still readable.
        let rotated = Envelope::new(
            "kek-b".to_owned(),
            Zeroizing::new([9u8; KEY_LEN]),
            Some(("kek-a".to_owned(), Zeroizing::new([7u8; KEY_LEN]))),
        );
        let (kek_id, rewrapped) = rotated
            .rewrap(&sealed.kek_id, &sealed.dek_wrapped)
            .expect("rewraps");
        assert_eq!(kek_id, "kek-b");

        // A later process holding only kek-b opens the untouched ciphertext.
        let only_new = Envelope::new("kek-b".to_owned(), Zeroizing::new([9u8; KEY_LEN]), None);
        let opened = only_new
            .open(&kek_id, &rewrapped, &sealed.nonce, &sealed.ciphertext)
            .expect("opens under the new master");
        assert_eq!(opened.as_slice(), b"survives rotation");

        // And the old wrap is unreadable to it, by id, not by accident.
        assert_eq!(
            only_new.open(
                "kek-a",
                &sealed.dek_wrapped,
                &sealed.nonce,
                &sealed.ciphertext
            ),
            Err(CryptoError::UnknownKek("kek-a".to_owned())),
        );
    }
}
