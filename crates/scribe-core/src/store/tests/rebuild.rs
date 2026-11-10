use super::private_tempdir;
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
        .settle_pass(&id, Some(original.clone()), false, &redactor)
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
        .settle_pass(&id, Some(advanced.clone()), false, &redactor)
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
