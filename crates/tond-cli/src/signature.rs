// signature.rs — release signature verification (minisign)
//
// Release tarballs are signed in the release workflow with the project's
// minisign key (release/SIGNING.md). The public key is compiled in from
// release/minisign.pub, and the same key is embedded in install.sh,
// update.sh and the tond.io web scripts (a test below keeps them in sync).
//
// Policy (the same in every script):
//   - a signature that is present must verify, always
//   - a missing signature is refused ("fail closed"), except for releases
//     published before signing existed (UNSIGNED_RELEASES) or when the admin
//     sets TOND_ALLOW_UNSIGNED=1 on purpose
//   - a build without a configured key (the placeholder file) cannot verify
//     anything; the release workflow refuses to publish from such a build
//
//   release_public_key()   the embedded key, if configured
//   check_release()        apply the policy to a downloaded tarball

use anyhow::{Result, bail};
use minisign_verify::{PublicKey, Signature};

/// release/minisign.pub: an untrusted-comment line, then the base64 key
/// (empty until the maintainer configures a key).
const RELEASE_PUBLIC_KEY_FILE: &str = include_str!("../../../release/minisign.pub");

/// Releases published before signing existed.
pub const UNSIGNED_RELEASES: &[&str] = &["v0.1.0"];

/// Environment variable that allows installing an unsigned release.
pub const ALLOW_UNSIGNED_ENV: &str = "TOND_ALLOW_UNSIGNED";

/// The base64 public key line of a minisign public key file, if present.
pub fn key_line(file: &str) -> Option<&str> {
    file.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))
}

/// The embedded release public key, or `None` when this build has none.
pub fn release_public_key() -> Option<&'static str> {
    key_line(RELEASE_PUBLIC_KEY_FILE)
}

/// What happened to a downloaded release.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The signature verified with the release key.
    Verified,
    /// Allowed without a signature (legacy release, override, or no key in
    /// this build); the string says why, for a warning.
    Unverified(String),
}

/// Apply the signature policy to `tarball` of release `tag` (e.g. "v0.2.0").
/// `signature` is the `.minisig` file's content, or `None` when the release
/// has no signature.
pub fn check_release(
    tarball: &[u8],
    signature: Option<&str>,
    tag: &str,
    public_key: Option<&str>,
    allow_unsigned: bool,
) -> Result<Verdict> {
    let Some(key) = public_key else {
        return Ok(Verdict::Unverified(
            "this tond build has no release signing key, so the download's signature can't be \
             checked (only its checksum)"
                .to_string(),
        ));
    };
    match signature {
        Some(sig) => {
            let pk = PublicKey::from_base64(key)
                .map_err(|e| anyhow::anyhow!("the embedded release key is invalid: {e}"))?;
            let sig = Signature::decode(sig)
                .map_err(|e| anyhow::anyhow!("the release signature is malformed: {e}"))?;
            if let Err(e) = pk.verify(tarball, &sig, false) {
                bail!(
                    "the signature of release {tag} does not verify ({e}). The download may have \
                     been tampered with; nothing was installed."
                );
            }
            Ok(Verdict::Verified)
        }
        None if UNSIGNED_RELEASES.contains(&tag) => Ok(Verdict::Unverified(format!(
            "{tag} was published before releases were signed"
        ))),
        None if allow_unsigned => Ok(Verdict::Unverified(format!(
            "{ALLOW_UNSIGNED_ENV}=1 is set, so the missing signature of {tag} was ignored"
        ))),
        None => bail!(
            "release {tag} has no signature (.minisig), so it can't be verified and was not \
             installed. If you trust this download anyway, rerun with {ALLOW_UNSIGNED_ENV}=1."
        ),
    }
}

/// Whether the admin opted into unsigned releases.
pub fn allow_unsigned_from_env() -> bool {
    std::env::var(ALLOW_UNSIGNED_ENV).is_ok_and(|v| v == "1")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestKey {
        pk_b64: String,
        kp: minisign::KeyPair,
    }

    fn test_key() -> TestKey {
        let kp = minisign::KeyPair::generate_unencrypted_keypair()
            .unwrap_or_else(|e| panic!("keypair: {e}"));
        TestKey {
            pk_b64: kp.pk.to_base64(),
            kp,
        }
    }

    fn sign(key: &TestKey, data: &[u8]) -> String {
        minisign::sign(
            Some(&key.kp.pk),
            &key.kp.sk,
            std::io::Cursor::new(data),
            Some("tond v0.2.0"),
            None,
        )
        .unwrap_or_else(|e| panic!("sign: {e}"))
        .into_string()
    }

    #[test]
    fn a_valid_signature_verifies() {
        let key = test_key();
        let data = b"tarball bytes";
        let sig = sign(&key, data);
        assert_eq!(
            check_release(data, Some(&sig), "v0.2.0", Some(&key.pk_b64), false)
                .unwrap_or_else(|e| panic!("should verify: {e}")),
            Verdict::Verified
        );
    }

    #[test]
    fn a_tampered_tarball_or_foreign_key_is_refused_even_with_the_override() {
        let key = test_key();
        let other = test_key();
        let sig = sign(&key, b"original");
        assert!(check_release(b"tampered", Some(&sig), "v0.2.0", Some(&key.pk_b64), true).is_err());
        assert!(
            check_release(b"original", Some(&sig), "v0.2.0", Some(&other.pk_b64), true).is_err(),
            "a signature by another key must not verify"
        );
        assert!(
            check_release(
                b"original",
                Some("garbage"),
                "v0.2.0",
                Some(&key.pk_b64),
                true
            )
            .is_err()
        );
    }

    #[test]
    fn a_missing_signature_fails_closed_unless_legacy_or_overridden() {
        let key = test_key();
        let err = check_release(b"x", None, "v0.2.0", Some(&key.pk_b64), false)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            err.contains(ALLOW_UNSIGNED_ENV),
            "the error names the override: {err}"
        );
        assert!(matches!(
            check_release(b"x", None, "v0.1.0", Some(&key.pk_b64), false),
            Ok(Verdict::Unverified(_))
        ));
        assert!(matches!(
            check_release(b"x", None, "v0.2.0", Some(&key.pk_b64), true),
            Ok(Verdict::Unverified(_))
        ));
    }

    #[test]
    fn key_line_skips_the_comment_and_blank_lines() {
        assert_eq!(key_line("untrusted comment: x\nRWabc\n"), Some("RWabc"));
        assert_eq!(key_line("untrusted comment: not configured\n\n"), None);
    }

    /// install.sh, update.sh and the web scripts embed the same key as
    /// release/minisign.pub (TOND_RELEASE_PUBKEY="...").
    #[test]
    fn scripts_embed_the_release_key() {
        let expected = release_public_key().unwrap_or("");
        for (name, script) in [
            (
                "release/install.sh",
                include_str!("../../../release/install.sh"),
            ),
            (
                "release/update.sh",
                include_str!("../../../release/update.sh"),
            ),
            (
                "release/web/install.sh",
                include_str!("../../../release/web/install.sh"),
            ),
            (
                "release/web/update.sh",
                include_str!("../../../release/web/update.sh"),
            ),
        ] {
            let embedded = script
                .lines()
                .find_map(|l| l.trim().strip_prefix("TOND_RELEASE_PUBKEY="))
                .map(|v| v.trim_matches('"'))
                .unwrap_or_else(|| panic!("{name} has no TOND_RELEASE_PUBKEY line"));
            assert_eq!(
                embedded, expected,
                "{name} must embed the key from release/minisign.pub"
            );
        }
    }
}
