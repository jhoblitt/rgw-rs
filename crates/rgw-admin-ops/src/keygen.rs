//! Access key generation, as `RGWAccessKeyPool::generate_key` does it with
//! `gen_rand_alphanumeric_upper` and `gen_rand_base64`.

use rand::RngExt;

/// `PUBLIC_ID_LEN` in `rgw_user.h`.
pub const ACCESS_KEY_LEN: usize = 20;
/// `SECRET_KEY_LEN` in `rgw_user.h`.
pub const SECRET_KEY_LEN: usize = 40;

const ACCESS_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
const SECRET_ALPHABET: &[u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn random_string(alphabet: &[u8], len: usize) -> String {
    let mut rng = rand::rng();
    (0..len).map(|_| alphabet[rng.random_range(0..alphabet.len())] as char).collect()
}

/// `gen_rand_alphanumeric_upper(cct, id, PUBLIC_ID_LEN)`: 20 chars of `A-Z0-9`.
pub fn generate_access_key() -> String {
    random_string(ACCESS_ALPHABET, ACCESS_KEY_LEN)
}

/// `gen_rand_base64(cct, secret, SECRET_KEY_LEN)`: 40 chars of `A-Za-z0-9+/`.
pub fn generate_secret_key() -> String {
    random_string(SECRET_ALPHABET, SECRET_KEY_LEN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_key_is_20_upper_alnum() {
        for _ in 0..100 {
            let k = generate_access_key();
            assert_eq!(k.len(), ACCESS_KEY_LEN);
            assert!(k.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()), "{k}");
        }
    }

    #[test]
    fn secret_key_is_40_base64_chars() {
        for _ in 0..100 {
            let k = generate_secret_key();
            assert_eq!(k.len(), SECRET_KEY_LEN);
            // The message deliberately omits the key: it is a secret, even here.
            assert!(k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/'));
        }
    }

    #[test]
    fn keys_differ() {
        assert_ne!(generate_access_key(), generate_access_key());
        assert_ne!(generate_secret_key(), generate_secret_key());
    }
}
