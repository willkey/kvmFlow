use sha2::{Digest, Sha256};

pub const PAIRING_CODE_DIGITS: usize = 6;

pub fn derive_pairing_code(
    tls_exporter: &[u8],
    windows_public_key: &[u8],
    mac_public_key: &[u8],
    windows_nonce: &[u8],
    mac_nonce: &[u8],
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"kvmflow-pairing-code-v1");
    digest.update(tls_exporter);
    digest.update(windows_public_key);
    digest.update(mac_public_key);
    digest.update(windows_nonce);
    digest.update(mac_nonce);
    let bytes = digest.finalize();
    let value = u32::from_be_bytes(bytes[..4].try_into().expect("SHA-256 prefix")) % 1_000_000;
    format!("{value:0PAIRING_CODE_DIGITS$}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_code_is_stable_and_six_digits() {
        let first = derive_pairing_code(b"exporter", b"windows", b"mac", b"wn", b"mn");
        let second = derive_pairing_code(b"exporter", b"windows", b"mac", b"wn", b"mn");

        assert_eq!(first, second);
        assert_eq!(first.len(), PAIRING_CODE_DIGITS);
        assert!(first.bytes().all(|byte| byte.is_ascii_digit()));
    }

    #[test]
    fn transcript_changes_produce_different_codes() {
        let first = derive_pairing_code(b"exporter", b"windows", b"mac", b"wn", b"mn");
        let second = derive_pairing_code(b"exporter", b"windows", b"mac", b"wn", b"other");

        assert_ne!(first, second);
    }
}
