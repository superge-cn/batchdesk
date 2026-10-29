//! ULID-shaped ids for plans/tasks. The kernel stays pure; the shell supplies
//! time/entropy through this helper.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
static PLAN_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn new_plan_id(root: &Path) -> String {
    new_ulid(&root.to_string_lossy())
}

pub fn new_ulid(seed: &str) -> String {
    let now = SystemTime::now();
    let since_epoch = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let millis = (since_epoch.as_millis() as u64) & 0xffff_ffff_ffff;
    let counter = PLAN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let entropy_source = format!(
        "{}:{}:{}:{}",
        since_epoch.as_nanos(),
        std::process::id(),
        counter,
        seed
    );
    let digest = blake3::hash(entropy_source.as_bytes());
    let mut bytes = [0u8; 16];
    bytes[..6].copy_from_slice(&millis.to_be_bytes()[2..]);
    bytes[6..].copy_from_slice(&digest.as_bytes()[..10]);
    let mut number = u128::from_be_bytes(bytes);
    let mut output = [0u8; 26];
    for slot in output.iter_mut().rev() {
        *slot = CROCKFORD[(number & 0x1f) as usize];
        number >>= 5;
    }
    String::from_utf8(output.to_vec()).expect("Crockford alphabet is ASCII")
}

pub fn is_ulid(value: &str) -> bool {
    value.len() == 26
        && value.as_bytes()[0] <= b'7'
        && value.bytes().all(|byte| CROCKFORD.contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_plan_id_has_ulid_shape() {
        let plan_id = new_plan_id(Path::new("."));
        assert!(is_ulid(&plan_id));
    }
}
