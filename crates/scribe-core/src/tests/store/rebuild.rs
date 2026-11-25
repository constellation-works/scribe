use super::private_tempdir;
use crate::ProposalVerbs;
use crate::apps::session::{Finished, Mark};
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
    let mut rebuilt = original.clone();
    rebuilt.summary = String::from("Rebuilt header");
    rebuilt.log[0].origin = SegmentOrigin::Rebuild;
    assert!(
        store
            .checkpoint_rebuild(rebuilt.clone(), None, &redactor)
            .unwrap()
            .is_some()
    );
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
    // The visible record advanced: the checkpoint is rebased onto it.
    let mark = Mark::of(&rebuilt);
    assert!(matches!(
        store
            .finish_rebuild(rebuilt.clone(), &mark, &redactor)
            .unwrap(),
        Finished::Rebased(at) if at == advanced.updated
    ));
    assert_eq!(store.session(&id).unwrap().record, advanced);
    let mut rebased = rebuilt.clone();
    rebased.updated = advanced.updated;
    let mark = Mark::of(&rebased);
    rebuilt.updated += time::Duration::seconds(2);
    let Finished::Written(written) = store.finish_rebuild(rebuilt, &mark, &redactor).unwrap()
    else {
        panic!("the rebased checkpoint replaces the record");
    };
    assert!(!written.active);
    assert!(written.keep_active);
    assert_eq!(written.created, original.created);
    assert_eq!(written.summary, "Rebuilt header");
    assert_eq!(written.log[0].origin, SegmentOrigin::Rebuild);
    assert!(store.rebuild_checkpoint(&written).unwrap().is_none());
}

#[test]
fn a_forgotten_session_takes_no_checkpoint_and_no_finish() {
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
    let err = store
        .checkpoint_rebuild(rebuilt.clone(), None, &redactor)
        .unwrap_err();
    assert_eq!(err.code(), "session_forgotten");
    assert_eq!(store.rebuild_checkpoint(&original).unwrap(), None);
    let err = store
        .finish_rebuild(rebuilt.clone(), &Mark::of(&rebuilt), &redactor)
        .unwrap_err();
    assert_eq!(err.code(), "session_forgotten");
}
