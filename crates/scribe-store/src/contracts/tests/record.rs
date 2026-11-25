use crate::record::{Label, Listing, Project, Related, RelatedKind, Topic, Topics};
use std::num::NonZeroUsize;

#[test]
fn projects_and_labels_are_one_token() {
    assert!("constellation-works/scribe".parse::<Project>().is_ok());
    assert!("global".parse::<Project>().is_ok());
    for bad in [
        "",
        "two words",
        "tab\there",
        "line\nbreak",
        &"x".repeat(129),
    ] {
        assert!(bad.parse::<Project>().is_err(), "{bad:?}");
        assert!(Label::try_from(bad.to_owned()).is_err(), "{bad:?}");
    }
}

#[test]
fn topics_are_kebab_case_at_most_five_and_never_repeated() {
    assert!("crew-routing".parse::<Topic>().is_ok());
    for bad in ["CI", "ci tools", "ci_tools", "-ci", ""] {
        assert!(bad.parse::<Topic>().is_err(), "{bad:?}");
    }
    let topics = |names: &[&str]| -> Result<Topics, _> {
        Topics::try_from(
            names
                .iter()
                .map(|n| n.parse::<Topic>().unwrap())
                .collect::<Vec<_>>(),
        )
    };
    assert!(topics(&["a", "b", "c", "d", "e"]).is_ok());
    assert!(topics(&["a", "b", "c", "d", "e", "f"]).is_err());
    assert!(topics(&["a", "a"]).is_err());
}

#[test]
fn related_references_need_a_known_kind_and_a_target() {
    let pr = Related::try_from(String::from("pr:constellation-works/scribe#7")).unwrap();
    assert_eq!(pr.kind(), RelatedKind::Pr);
    assert_eq!(pr.target(), "constellation-works/scribe#7");
    assert_eq!(pr.to_string(), "pr:constellation-works/scribe#7");
    let doc = "doc:scribe/docs/design/scribe/4_decisions.md#agent-records";
    let parsed: Related = doc.parse().unwrap();
    assert_eq!(parsed.kind(), RelatedKind::Doc);
    assert_eq!(parsed.to_string(), doc);
    assert_eq!(
        serde_json::from_str::<Related>(&serde_json::to_string(&parsed).unwrap()).unwrap(),
        parsed
    );
    for good in ["task:<id>", "repo:scribe", "session:abc"] {
        assert!(Related::try_from(good.to_owned()).is_ok(), "{good}");
    }
    for bad in [
        "issue:3",
        "task:",
        "scribe",
        "repo:two words",
        "doc:scribe#anchor",
        "doc:scribe/file.md",
        "doc:scribe/file.md#",
        "doc:scribe/#anchor",
    ] {
        assert!(Related::try_from(bad.to_owned()).is_err(), "{bad}");
    }
}

#[test]
fn a_listing_counts_every_match_before_the_limit() {
    let listing = Listing::select(vec![1, 2, 3, 4, 5], |n| n % 2 == 1, NonZeroUsize::new(2));
    assert_eq!(listing.items, [1, 3]);
    assert_eq!(listing.total, 3);
    assert!(listing.truncated());
    let whole = Listing::select(vec![1, 2], |_| true, None);
    assert!(!whole.truncated());
}

#[test]
fn reserved_author_names_cover_every_internal_channel() {
    for reserved in [
        "user",
        "daniel",
        "scribe",
        "consistency",
        "session-pass",
        "tag-retired",
    ] {
        let name: crate::record::AuthorName = reserved.parse().unwrap();
        assert!(name.is_reserved(), "{reserved}");
    }
    let agent: crate::record::AuthorName = "opus".parse().unwrap();
    assert!(!agent.is_reserved());
}
