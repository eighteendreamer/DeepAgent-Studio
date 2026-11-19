use std::sync::Arc;

use deepagent_persistence::Database;
use deepagent_ssh::{SshHostIdentity, SshHostKeyStore};
use russh::keys::{Algorithm, HashAlg, PrivateKey};

fn key() -> russh::keys::PublicKey {
    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
        .unwrap()
        .public_key()
        .clone()
}

#[test]
fn verified_keys_survive_reopen_and_revocation_is_a_durable_tombstone() {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("app.sqlite");
    let identity = SshHostIdentity::direct("EXAMPLE.invalid", 2222).unwrap();
    let public = key();
    let fingerprint = public.fingerprint(HashAlg::Sha256).to_string();
    {
        let store = SshHostKeyStore::new(Arc::new(Database::open(&path).unwrap()));
        store
            .trust_verified(&identity, &public, "verified fixture")
            .unwrap();
        store
            .trust_verified(&identity, &public, "duplicate")
            .unwrap();
        let listed = store.list(&identity).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].fingerprint, fingerprint);
        assert_eq!(listed[0].source, "verified fixture");
        assert!(!listed[0].revoked);
    }
    let store = SshHostKeyStore::new(Arc::new(Database::open(&path).unwrap()));
    assert_eq!(store.list(&identity).unwrap().len(), 1);
    store.revoke(&identity, &fingerprint).unwrap();
    assert!(store.list(&identity).unwrap()[0].revoked);
    assert!(store
        .trust_verified(&identity, &public, "must not revive")
        .is_err());
    assert!(store
        .trust_verified(&identity, &key(), "must not rotate silently")
        .is_err());
    let reopened = SshHostKeyStore::new(Arc::new(Database::open(&path).unwrap()));
    assert!(reopened.list(&identity).unwrap()[0].revoked);
}

#[test]
fn normalized_endpoint_and_port_are_part_of_the_trust_identity() {
    let store = SshHostKeyStore::new(Arc::new(Database::open_in_memory().unwrap()));
    let identity = SshHostIdentity::direct("  EXAMPLE.invalid  ", 22).unwrap();
    store.trust_verified(&identity, &key(), "fixture").unwrap();
    assert_eq!(
        store
            .list(&SshHostIdentity::direct("example.invalid", 22).unwrap())
            .unwrap()
            .len(),
        1
    );
    assert!(store
        .list(&SshHostIdentity::direct("example.invalid", 2222).unwrap())
        .unwrap()
        .is_empty());
    assert!(store
        .list(&SshHostIdentity::direct("other.invalid", 22).unwrap())
        .unwrap()
        .is_empty());
    assert_eq!(
        SshHostIdentity::direct("0:0:0:0:0:0:0:1", 22).unwrap(),
        SshHostIdentity::direct("::1", 22).unwrap()
    );
    assert!(SshHostIdentity::direct("", 22).is_err());
    assert!(SshHostIdentity::direct("example.invalid", 0).is_err());
}

#[test]
fn two_database_handles_cannot_overwrite_a_conflicting_verified_key() {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("app.sqlite");
    let one = SshHostKeyStore::new(Arc::new(Database::open(&path).unwrap()));
    let two = SshHostKeyStore::new(Arc::new(Database::open(&path).unwrap()));
    let identity = SshHostIdentity::direct("fixture.invalid", 22).unwrap();
    let barrier = std::sync::Barrier::new(2);
    let keys = [key(), key()];
    let results = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            one.trust_verified(&identity, &keys[0], "one")
        });
        let b = scope.spawn(|| {
            barrier.wait();
            two.trust_verified(&identity, &keys[1], "two")
        });
        [a.join().unwrap(), b.join().unwrap()]
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(one.list(&identity).unwrap().len(), 1);
}

#[test]
fn public_key_comments_are_not_a_new_identity_or_a_conflict() {
    let store = SshHostKeyStore::new(Arc::new(Database::open_in_memory().unwrap()));
    let identity = SshHostIdentity::direct("fixture.invalid", 22).unwrap();
    let mut public = key();
    public.set_comment("first local label");
    store.trust_verified(&identity, &public, "fixture").unwrap();
    public.set_comment("different local label");
    store
        .trust_verified(&identity, &public, "same public key")
        .unwrap();
    assert_eq!(store.list(&identity).unwrap().len(), 1);
}
