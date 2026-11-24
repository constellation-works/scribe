use super::private_tempdir;
use crate::ProposalVerbs;
use crate::consistency::Settle;
use crate::entry::Cite;
use crate::testing::ViewTestReads;
use crate::{PassVerbs, RebuildVerbs};
use crate::{Redactor, SegmentOrigin, SegmentState, SessionFilter, Store};
use time::macros::datetime;

#[test]
fn rebuild_checkpoints_are_hidden_and_replacement_compares_the_visible_revision() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let redactor = Redactor::new([]);
    let original = crate::tests::session("s-1", datetime!(2026-11-01 10:00 UTC));
    let id = original.id.clone();
    store
        .settle_pass(&id, Some(original.clone()), false, None, &redactor)
        .unwrap();
    let _guard = store.lock_rebuild(&original).unwrap();
    let mut rebuilt = original.clone();
    rebuilt.summary = String::from("Rebuilt header");
    rebuilt.log[0].origin = SegmentOrigin::Rebuild;
    store
        .checkpoint_rebuild(rebuilt.clone(), &redactor)
        .unwrap();
    assert_eq!(store.session(&id).unwrap().record, original);
    assert_eq!(
        store
            .list_sessions(&SessionFilter::default())
            .unwrap()
            .listing
            .items
            .len(),
        1
    );
    assert_eq!(
        store.rebuild_checkpoint(&original).unwrap(),
        Some(rebuilt.clone())
    );
    let mut advanced = original.clone();
    advanced.updated += time::Duration::seconds(1);
    advanced.log[0].state = SegmentState::Sealed;
    advanced.active = false;
    advanced.keep_active = true;
    store
        .settle_pass(
            &id,
            Some(advanced.clone()),
            false,
            Some(original.updated),
            &redactor,
        )
        .unwrap();
    assert!(
        store
            .finish_rebuild(rebuilt.clone(), original.updated, &redactor)
            .unwrap()
            .is_none()
    );
    assert_eq!(store.session(&id).unwrap().record, advanced);
    rebuilt.updated += time::Duration::seconds(2);
    let written = store
        .finish_rebuild(rebuilt, advanced.updated, &redactor)
        .unwrap()
        .unwrap();
    assert!(!written.active);
    assert!(written.keep_active);
    assert_eq!(written.created, original.created);
    assert_eq!(written.summary, "Rebuilt header");
    assert_eq!(written.log[0].origin, SegmentOrigin::Rebuild);
    assert!(store.rebuild_checkpoint(&written).unwrap().is_none());
}

#[test]
fn a_forgotten_session_takes_no_rebuild_lock_and_no_checkpoint() {
    let dir = private_tempdir();
    let store = Store::open(dir.path());
    let redactor = Redactor::new([]);
    let original = crate::tests::session("s-1", datetime!(2026-11-01 10:00 UTC));
    let id = original.id.clone();
    store
        .settle_pass(&id, Some(original.clone()), false, None, &redactor)
        .unwrap();
    let cite = Cite {
        session: id.clone(),
        message: None,
        at: datetime!(2026-11-01 10:05 UTC),
        prompt_sha256: "0".repeat(64),
        line: 1,
        cwd: String::from("/work"),
    };
    store
        .settle(
            &Settle::ForgetSession(id.clone()),
            &cite,
            datetime!(2026-11-01 10:06 UTC),
        )
        .unwrap();
    let mut rebuilt = original.clone();
    rebuilt.summary = String::from("Rebuilt header");
    let err = store.checkpoint_rebuild(rebuilt, &redactor).unwrap_err();
    assert_eq!(err.code(), "session_forgotten");
    assert_eq!(store.rebuild_checkpoint(&original).unwrap(), None);
    let err = store.lock_rebuild(&original).unwrap_err();
    assert_eq!(err.code(), "session_forgotten");
}
