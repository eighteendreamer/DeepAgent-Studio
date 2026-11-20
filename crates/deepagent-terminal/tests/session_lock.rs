use deepagent_terminal::{TerminalError, TerminalInputHolder, TerminalLeaseRegistry};

#[test]
fn lock_fences_every_input_holder_but_preserves_read_scope_and_renewal() {
    let registry = TerminalLeaseRegistry::default();
    let lease = registry
        .register("pty", "run", TerminalInputHolder::User)
        .unwrap();
    registry.lock(&lease, "fixture password", true).unwrap();
    assert!(matches!(
        registry.validate_input(&lease),
        Err(TerminalError::Locked)
    ));
    assert!(registry.validate(&lease).is_ok());
    assert!(registry.validate_scope("pty", "run").is_ok());
    assert_eq!(registry.lock_status("pty").unwrap(), Some(true));
    assert!(registry
        .takeover("pty", TerminalInputHolder::Runtime)
        .is_err());
    assert!(registry.unlock(&lease, "wrong").is_err());
    assert!(registry.validate_input(&lease).is_err());
    registry.unlock(&lease, "fixture password").unwrap();
    registry.validate_input(&lease).unwrap();
    assert_eq!(registry.lock_status("pty").unwrap(), None);
    let next = registry
        .takeover("pty", TerminalInputHolder::Runtime)
        .unwrap();
    assert!(registry.lock(&lease, "stale", false).is_err());
    registry.validate_input(&next).unwrap();
}

#[test]
fn lock_is_scoped_and_cannot_be_replaced_or_cleared_with_a_stale_owner() {
    let registry = TerminalLeaseRegistry::default();
    let a = registry
        .register("a", "run-a", TerminalInputHolder::User)
        .unwrap();
    let b = registry
        .register("b", "run-b", TerminalInputHolder::User)
        .unwrap();
    assert!(registry.lock(&a, "", false).is_err());
    registry.lock(&a, "first", false).unwrap();
    assert!(registry.lock(&a, "overwrite", true).is_err());
    registry.validate_input(&b).unwrap();
    assert!(registry.unlock(&b, "first").is_ok());
    assert!(registry.validate_input(&a).is_err());
    registry.unlock(&a, "first").unwrap();
    registry.remove(&a).unwrap();
    let successor = registry
        .register("a", "run-next", TerminalInputHolder::User)
        .unwrap();
    assert!(registry.unlock(&a, "first").is_err());
    registry.validate_input(&successor).unwrap();
}
