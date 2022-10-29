use std::{
    collections::HashMap,
    io::{self, Error, ErrorKind},
};

use crate::{
    formatting,
    ids::short,
    key::{
        self,
        secp256k1::{self, public_key::Key as PublicKey, signature::Sig},
    },
};
use async_trait::async_trait;
use k256::ecdsa::signature::hazmat::PrehashSigner;
use lazy_static::lazy_static;
use rand::{seq::SliceRandom, thread_rng};
use ring::rand::{SecureRandom, SystemRandom};

/// The size (in bytes) of a secret key.
/// ref. "secp256k1::constants::SECRET_KEY_SIZE"
pub const LEN: usize = 32;

pub const HEX_ENCODE_PREFIX: &str = "0x";
pub const CB58_ENCODE_PREFIX: &str = "PrivateKey-";

/// Represents "k256::SecretKey" and "k256::ecdsa::SigningKey".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key(k256::SecretKey);

fn secure_random() -> &'static dyn SecureRandom {
    use std::ops::Deref;
    lazy_static! {
        static ref RANDOM: SystemRandom = SystemRandom::new();
    }
    RANDOM.deref()
}

impl Key {
    /// Generates a private key from random bytes.
    pub fn generate() -> io::Result<Self> {
        let mut b = [0u8; LEN];
        secure_random()
            .fill(&mut b)
            .map_err(|e| Error::new(ErrorKind::Other, format!("failed secure_random {}", e)))?;
        Self::from_bytes(&b)
    }

    /// Loads the private key from the raw scalar bytes.
    pub fn from_bytes(raw: &[u8]) -> io::Result<Self> {
        assert_eq!(raw.len(), LEN);
        let sk = k256::SecretKey::from_be_bytes(raw).map_err(|e| {
            Error::new(
                ErrorKind::Other,
                format!("failed k256::SecretKey::from_be_bytes {}", e),
            )
        })?;
        Ok(Self(sk))
    }

    pub fn signing_key(&self) -> k256::ecdsa::SigningKey {
        k256::ecdsa::SigningKey::from(self.0.clone())
    }

    /// Converts the private key to raw scalar bytes.
    pub fn to_bytes(&self) -> [u8; LEN] {
        let b = self.0.to_be_bytes();

        let mut bb = [0u8; LEN];
        bb.copy_from_slice(&b);
        bb
    }

    /// Hex-encodes the raw private key to string with "0x" prefix (e.g., Ethereum).
    pub fn to_hex(&self) -> String {
        let b = self.0.to_be_bytes();
        let enc = hex::encode(&b);

        let mut s = String::from(HEX_ENCODE_PREFIX);
        s.push_str(&enc);
        s
    }

    /// Loads the private key from a hex-encoded string (e.g., Ethereum).
    pub fn from_hex<S>(s: S) -> io::Result<Self>
    where
        S: Into<String>,
    {
        let ss: String = s.into();
        let ss = ss.trim_start_matches(HEX_ENCODE_PREFIX);

        let b = hex::decode(ss)
            .map_err(|e| Error::new(ErrorKind::Other, format!("failed hex::decode {}", e)))?;
        Self::from_bytes(&b)
    }

    /// Encodes the raw private key to string with "PrivateKey-" prefix (e.g., Avalanche).
    pub fn to_cb58(&self) -> String {
        let b = self.0.to_be_bytes();
        let enc = formatting::encode_cb58_with_checksum_string(&b);

        let mut s = String::from(CB58_ENCODE_PREFIX);
        s.push_str(&enc);
        s
    }

    /// Loads the private key from a CB58-encoded string (e.g., Avalanche).
    /// Once decoded and with its "PrivateKey-" prefix removed,
    /// the length must be 32-byte.
    pub fn from_cb58<S>(s: S) -> io::Result<Self>
    where
        S: Into<String>,
    {
        let ss: String = s.into();
        let ss = ss.trim_start_matches(CB58_ENCODE_PREFIX);

        let b = formatting::decode_cb58_with_checksum(ss)?;
        Self::from_bytes(&b)
    }

    /// Derives the public key from this private key.
    pub fn to_public_key(&self) -> PublicKey {
        PublicKey::from(self.0.public_key())
    }

    /// Converts to Info.
    pub fn to_info(&self, network_id: u32) -> io::Result<key::secp256k1::Info> {
        let pk_cb58 = self.to_cb58();
        let pk_hex = self.to_hex();

        let pubkey = self.to_public_key();
        let short_addr = pubkey.to_short_id()?;
        let eth_addr = pubkey.eth_address();

        let mut addresses = HashMap::new();
        let x_address = pubkey.hrp_address(network_id, "X")?;
        let p_address = pubkey.hrp_address(network_id, "P")?;
        let c_address = pubkey.hrp_address(network_id, "C")?;
        addresses.insert(
            network_id,
            secp256k1::ChainAddresses {
                x_address,
                p_address,
                c_address,
            },
        );

        Ok(key::secp256k1::Info {
            mnemonic_phrase: None,

            private_key_cb58: pk_cb58,
            private_key_hex: pk_hex,

            addresses,

            short_address: short_addr,
            eth_address: eth_addr,
        })
    }

    /// Signs the 32-byte SHA256 output message with the ECDSA private key and the recoverable code.
    /// "github.com/decred/dcrd/dcrec/secp256k1/v3/ecdsa.SignCompact" outputs 65-byte signature.
    /// ref. "avalanchego/utils/crypto.PrivateKeySECP256K1R.SignHash"
    /// ref. https://github.com/rust-bitcoin/rust-secp256k1/blob/master/src/ecdsa/recovery.rs
    pub fn sign_digest(&self, digest: &[u8]) -> io::Result<Sig> {
        // ref. "crypto/sha256.Size"
        assert_eq!(digest.len(), ring::digest::SHA256_OUTPUT_LEN);

        // NOTE
        // "k256::ecdsa::SigningKey::sign" with "k256::ecdsa::signature::Signer"
        // signs the message, not a message digest, so the message is first hashed
        // with Keccak256 in such case. Use "sign_prehash" since avalanche signs
        // the already hashed SHA256 output.
        // ref. https://github.com/RustCrypto/elliptic-curves/issues/671
        let sig: k256::ecdsa::recoverable::Signature = self
            .signing_key()
            .sign_prehash(digest)
            .map_err(|e| Error::new(ErrorKind::Other, format!("failed sign_prehash '{}'", e)))?;

        Ok(sig.into())
    }

    /// Derives the private key that uses libsecp256k1.
    #[cfg(feature = "libsecp256k1")]
    pub fn to_libsecp256k1(&self) -> io::Result<crate::key::secp256k1::libsecp256k1::PrivateKey> {
        let b = self.to_bytes();
        crate::key::secp256k1::libsecp256k1::PrivateKey::from_bytes(&b)
    }
}

impl From<k256::SecretKey> for Key {
    fn from(s: k256::SecretKey) -> Self {
        Self(s)
    }
}

impl From<Key> for k256::SecretKey {
    fn from(s: Key) -> Self {
        s.0
    }
}

/// ref. https://doc.rust-lang.org/std/string/trait.ToString.html
/// ref. https://doc.rust-lang.org/std/fmt/trait.Display.html
/// Use "Self.to_string()" to directly invoke this
impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.to_bytes()))
    }
}

#[async_trait]
impl key::secp256k1::SignOnly for Key {
    fn signing_key(&self) -> io::Result<k256::ecdsa::SigningKey> {
        Ok(self.signing_key())
    }

    async fn sign_digest(&self, msg: &[u8]) -> io::Result<[u8; 65]> {
        let sig = self.sign_digest(msg)?;
        Ok(sig.to_bytes())
    }
}

/// ref. https://doc.rust-lang.org/book/ch10-02-traits.html
impl key::secp256k1::ReadOnly for Key {
    fn hrp_address(&self, network_id: u32, chain_id_alias: &str) -> io::Result<String> {
        self.to_public_key().hrp_address(network_id, chain_id_alias)
    }

    fn short_address(&self) -> io::Result<short::Id> {
        self.to_public_key().to_short_id()
    }

    fn short_address_bytes(&self) -> io::Result<Vec<u8>> {
        self.to_public_key().to_short_bytes()
    }

    fn eth_address(&self) -> String {
        self.to_public_key().eth_address()
    }

    fn h160_address(&self) -> primitive_types::H160 {
        self.to_public_key().to_h160()
    }
}

/// RUST_LOG=debug cargo test --package avalanche-types --lib -- key::secp256k1::private_key::test_private_key --exact --show-output
#[test]
fn test_private_key() {
    use ring::digest::{digest, SHA256};

    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let msg: Vec<u8> = random_manager::bytes(100).unwrap();
    let hashed: Vec<u8> = digest(&SHA256, &msg).as_ref().into();

    let pk1 = Key::generate().unwrap();

    let sig1 = pk1.sign_digest(&hashed).unwrap();
    assert_eq!(sig1.to_bytes().len(), crate::key::secp256k1::signature::LEN);

    let raw_bytes = pk1.to_bytes();
    assert_eq!(raw_bytes.len(), LEN);

    let pk2 = Key::from_bytes(&raw_bytes).unwrap();
    assert_eq!(pk1, pk2);

    let hex1 = pk1.to_hex();
    let hex2 = pk2.to_hex();
    assert_eq!(hex1, hex2);
    log::info!("hex: {}", hex1);

    let pk3 = Key::from_hex(hex1).unwrap();
    assert_eq!(pk1, pk3);

    let cb1 = pk1.to_cb58();
    let cb2 = pk2.to_cb58();
    let cb3 = pk3.to_cb58();
    assert_eq!(cb1, cb2);
    assert_eq!(cb2, cb3);
    log::info!("cb58: {}", cb1);

    let pk4 = Key::from_cb58(cb1).unwrap();
    assert_eq!(pk1, pk2);
    assert_eq!(pk2, pk3);
    assert_eq!(pk3, pk4);
}

/// Loads keys from texts, assuming each key is line-separated.
/// Set "permute_keys" true to permute the key order from the contents "d".
pub fn load_cb58_keys(d: &[u8], permute_keys: bool) -> io::Result<Vec<Key>> {
    let text = match std::str::from_utf8(d) {
        Ok(s) => s,
        Err(e) => {
            return Err(Error::new(
                ErrorKind::Other,
                format!("failed to convert str from_utf8 {}", e),
            ));
        }
    };

    let mut lines = text.lines();
    let mut line_cnt = 1;

    let mut keys: Vec<Key> = Vec::new();
    let mut added = HashMap::new();
    loop {
        if let Some(s) = lines.next() {
            if added.get(s).is_some() {
                return Err(Error::new(
                    ErrorKind::Other,
                    format!("key at line {} already added before", line_cnt),
                ));
            }

            keys.push(Key::from_cb58(s).unwrap());

            added.insert(s, true);
            line_cnt += 1;
            continue;
        }
        break;
    }

    if permute_keys {
        keys.shuffle(&mut thread_rng());
    }
    Ok(keys)
}
