// spec-kit 001 — plugin signing CLI (dev/CI tool).
//
//   cargo run --example furx_sign -- gen-key
//       → prints base64 SIGNING_KEY (private, keep secret) + PUBKEY (pin in TRUSTED_PUBKEYS)
//   FURX_SIGN_KEY=<base64-privkey> cargo run --example furx_sign -- sign <plugin-dir>
//       → reads <dir>/manifest.json, fills entrypoint_sha256 + pubkey, signs, writes back
//
// Uses the crate's own SignedManifest::signing_bytes so the canonical bytes are
// byte-identical to what plugin_host verifies (no drift).

use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use furx_lib::services::plugin_host::{file_sha256, SignedManifest};

fn getrandom(buf: &mut [u8]) {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")
        .expect("urandom")
        .read_exact(buf)
        .expect("read urandom");
}

fn main() {
    let eng = base64::engine::general_purpose::STANDARD;
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()).unwrap_or("") {
        "gen-key" => {
            let mut seed = [0u8; 32];
            getrandom(&mut seed);
            let sk = SigningKey::from_bytes(&seed);
            println!("SIGNING_KEY={}", eng.encode(sk.to_bytes()));
            println!("PUBKEY={}", eng.encode(sk.verifying_key().to_bytes()));
            eprintln!("→ keep SIGNING_KEY secret (Keychain); pin PUBKEY in TRUSTED_PUBKEYS");
        }
        "sign" => {
            let dir = args.get(2).expect("usage: sign <plugin-dir>");
            let key_b64 =
                std::env::var("FURX_SIGN_KEY").expect("set FURX_SIGN_KEY=<base64 privkey>");
            let key_bytes: [u8; 32] = eng
                .decode(&key_b64)
                .expect("bad key b64")
                .try_into()
                .expect("key must be 32 bytes");
            let sk = SigningKey::from_bytes(&key_bytes);

            let dirp = std::path::Path::new(dir);
            let mpath = dirp.join("manifest.json");
            let text = std::fs::read_to_string(&mpath).expect("read manifest");
            let mut m: SignedManifest = serde_json::from_str(&text).expect("parse manifest");

            m.entrypoint_sha256 =
                Some(file_sha256(&dirp.join(&m.entrypoint)).expect("hash entrypoint"));
            m.pubkey = Some(eng.encode(sk.verifying_key().to_bytes()));
            m.signature = None;

            let bytes = m.signing_bytes().expect("signing bytes");
            m.signature = Some(eng.encode(sk.sign(&bytes).to_bytes()));

            std::fs::write(&mpath, serde_json::to_string_pretty(&m).unwrap())
                .expect("write manifest");
            println!("signed {}", mpath.display());
        }
        _ => {
            eprintln!("usage: furx_sign gen-key | (FURX_SIGN_KEY=.. furx_sign sign <plugin-dir>)");
            std::process::exit(2);
        }
    }
}
