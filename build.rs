use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::path::Path;

// TDLib is linked by tdlib-rs's own build script (its `static` feature), so
// this one doesn't use tdlib-rs: as a build dependency it would be built for
// the machine doing the build, which a cross-compiled release (Windows ARM64
// on an x64 runner) can't link against the target's TDLib.
fn main() {
    write_built_in_keys();
}

/// Writes the API key from `TUIGRAM_API_ID` / `TUIGRAM_API_HASH` (set by CI
/// for release builds) to `$OUT_DIR/built_in_keys.rs`, XORed with a mask that
/// is new every build. Anyone with the binary can still recover the key, but
/// it never appears as plain text for scanners looking for API hashes.
fn write_built_in_keys() {
    println!("cargo:rerun-if-env-changed=TUIGRAM_API_ID");
    println!("cargo:rerun-if-env-changed=TUIGRAM_API_HASH");
    let var = |name| std::env::var(name).unwrap_or_default().trim().to_string();
    let (id, hash) = (var("TUIGRAM_API_ID"), var("TUIGRAM_API_HASH"));
    let code = match (id.is_empty(), hash.is_empty()) {
        (true, true) => "const BUILT_IN_KEYS: Option<(&[u8], &[u8])> = None;\n".to_string(),
        (false, false) => {
            // The values themselves stay out of the message: build logs can
            // be public.
            let valid_id = id.parse::<i32>().is_ok_and(|id| id > 0);
            let valid_hash = hash.len() == 32 && hash.bytes().all(|b| b.is_ascii_hexdigit());
            assert!(
                valid_id && valid_hash,
                "TUIGRAM_API_ID must be a number and TUIGRAM_API_HASH 32 hex digits"
            );
            let plain = format!("{id}:{hash}");
            let mask = random_bytes(plain.len());
            let masked: Vec<u8> = plain.bytes().zip(&mask).map(|(b, m)| b ^ m).collect();
            format!(
                "const BUILT_IN_KEYS: Option<(&[u8], &[u8])> = Some((&{masked:?}, &{mask:?}));\n"
            )
        }
        _ => panic!("set both TUIGRAM_API_ID and TUIGRAM_API_HASH, or neither"),
    };
    let out = std::env::var("OUT_DIR").unwrap();
    std::fs::write(Path::new(&out).join("built_in_keys.rs"), code).unwrap();
}

/// Bytes that differ every build. `RandomState` is seeded randomly per
/// process, which is plenty for a mask that only has to hide a pattern.
fn random_bytes(len: usize) -> Vec<u8> {
    let state = RandomState::new();
    (0..len).map(|i| state.hash_one(i) as u8).collect()
}
