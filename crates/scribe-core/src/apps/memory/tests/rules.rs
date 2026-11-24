use super::T0;
use crate::entry::Speaker;
use crate::memory::{
    self, Checked, Draft, ElementKind, Milestone, NowItem, NowKind, PassChange, Pending,
    ProfileView, Refusal, profile,
};
use crate::models::{MemoryAnswer, PassMessage};
use crate::session::SessionId;
use std::collections::BTreeSet;
use time::Date;
use time::macros::date;

const TURN: &str = "My sister's wedding is on december 12 and I'm still on the seating.";
const QUOTE: &str = "My sister's wedding is on december 12";

fn user(uuid: &str, text: &str) -> PassMessage {
    PassMessage {
        uuid: uuid.to_owned(),
        role: Speaker::User,
        text: text.to_owned(),
        pasted: Vec::new(),
    }
}

fn element(kind: &str, title: &str, quote: &str) -> MemoryAnswer {
    MemoryAnswer {
        kind: kind.to_owned(),
        title: title.to_owned(),
        text: format!("{title}, as the user said."),
        quote: quote.to_owned(),
        message: String::from("u-1"),
        ..MemoryAnswer::default()
    }
}

fn never_masks(_: &str) -> bool {
    false
}

fn check_one(answer: MemoryAnswer, turn: &str) -> Result<Checked, Refusal> {
    let report = memory::check(&[answer], &[user("u-1", turn)], 3, &never_masks);
    assert_eq!(report.used, 1);
    match (report.checked.into_iter().next(), report.refused.first()) {
        (Some(checked), None) => Ok(checked),
        (None, Some(refusal)) => Err(*refusal),
        other => panic!("one outcome expected, got {other:?}"),
    }
}

fn session(n: u8) -> SessionId {
    format!("0b8ef4a6-0c6a-4d77-9c41-6a4f7b2c1d1{n}")
        .parse()
        .unwrap()
}

#[test]
fn a_verbatim_quote_of_the_users_turn_passes_and_anything_else_is_unverified() {
    let ok = check_one(element("now", "Sister's wedding", QUOTE), TURN).unwrap();
    assert_eq!(ok.kind, ElementKind::Now);
    assert_eq!(ok.now_kind, NowKind::Plan);
    assert_eq!(
        check_one(
            element("now", "Sister's wedding", "My brother's wedding"),
            TURN
        ),
        Err(Refusal::Unverified)
    );
    let mut other_turn = element("now", "Sister's wedding", QUOTE);
    other_turn.message = String::from("u-2");
    assert_eq!(check_one(other_turn, TURN), Err(Refusal::Unverified));
}

#[test]
fn shape_finance_and_mood_refuse_before_the_write() {
    assert_eq!(
        check_one(element("working", "Sister's wedding", QUOTE), TURN),
        Err(Refusal::Invalid)
    );
    assert_eq!(
        check_one(element("now", "", QUOTE), TURN),
        Err(Refusal::Invalid)
    );
    let mut kind = element("now", "Sister's wedding", QUOTE);
    kind.now_kind = Some(String::from("whim"));
    assert_eq!(check_one(kind, TURN), Err(Refusal::Invalid));
    let money = "I need to pay the mortgage by december 3";
    assert_eq!(
        check_one(element("now", "Pay the mortgage", money), money),
        Err(Refusal::Sensitive)
    );
    let mut mood = element("now", "Sister's wedding", QUOTE);
    mood.text = String::from("He is anxious about his sister's wedding.");
    assert_eq!(check_one(mood, TURN), Err(Refusal::Inferred));
    let report = memory::check(
        &[element("now", "Sister's wedding", QUOTE)],
        &[user("u-1", TURN)],
        3,
        &|text: &str| text.contains("wedding"),
    );
    assert_eq!(report.refused, [Refusal::Sensitive]);
}

#[test]
fn slots_bound_the_elements_taken() {
    let answers = vec![element("now", "Sister's wedding", QUOTE); 5];
    let report = memory::check(
        &answers,
        &[user("u-1", TURN)],
        memory::MAX_PER_PASS,
        &never_masks,
    );
    assert_eq!(report.used, 3);
    assert_eq!(report.checked.len(), 3);
}

fn draft(kind: ElementKind, title: &str, date: Option<Date>, turn: &str) -> Draft {
    Draft {
        checked: Checked {
            kind,
            title: title.to_owned(),
            text: format!("{title}."),
            quote: turn.to_owned(),
            message: String::from("u-1"),
            date: date.map(crate::record::nullable_date::format),
            now_kind: NowKind::Event,
            update: None,
            completes: None,
        },
        date,
        turn: turn.to_owned(),
        at: T0,
    }
}

#[test]
fn a_dated_now_element_writes_a_file_by_stem_and_an_undated_one_is_kept_pending() {
    let s = session(1);
    let dated = draft(
        ElementKind::Now,
        "Sister's wedding",
        Some(date!(2026 - 12 - 12)),
        TURN,
    );
    let undated = draft(ElementKind::Now, "Book the venue", None, TURN);
    let (changes, refused) = memory::plan(&[dated, undated.clone()], &[], &[], &[], &s);
    assert!(refused.is_empty());
    let PassChange::Now(item, cites) = &changes[0] else {
        panic!("a now file first, got {changes:?}")
    };
    assert_eq!(cites, &[(s.clone(), String::from("u-1"))]);
    assert_eq!(item.stem.as_str(), "2026-12-12-sister-s-wedding");
    assert_eq!(item.sources, std::slice::from_ref(&s));
    assert!(
        matches!(&changes[1], PassChange::AddPending { subject, .. } if subject == "Book the venue")
    );
    // The same subject again adds nothing.
    let pending = vec![Pending {
        id: 4,
        session: s.clone(),
        message: String::from("u-1"),
        subject: String::from("Book the venue"),
        text: String::from("Book the venue."),
    }];
    let (changes, _) = memory::plan(&[undated], &[], &[], &pending, &s);
    assert!(changes.is_empty());
}

#[test]
fn a_dated_completion_drops_its_candidate_and_names_its_session() {
    let first = session(1);
    let second = session(2);
    let pending = vec![Pending {
        id: 4,
        session: first.clone(),
        message: String::from("u-1"),
        subject: String::from("Book the venue"),
        text: String::from("Book the venue."),
    }];
    let mut done = draft(
        ElementKind::Now,
        "Book the venue",
        Some(date!(2026 - 11 - 20)),
        TURN,
    );
    done.checked.completes = Some(4);
    // The candidate belongs to another session: unknown here.
    let (_, refused) = memory::plan(std::slice::from_ref(&done), &[], &[], &pending, &second);
    assert_eq!(refused, [Refusal::Unknown]);
    let (changes, refused) = memory::plan(&[done], &[], &[], &pending, &first);
    assert!(refused.is_empty());
    assert_eq!(changes[0], PassChange::DropPending(4));
    let PassChange::Now(n, cites) = &changes[1] else {
        panic!("a now file, got {changes:?}")
    };
    assert_eq!(n.sources, std::slice::from_ref(&first));
    // It cites the dating turn and the candidate's own turn.
    assert_eq!(cites.len(), 2);
    let mut undated = draft(ElementKind::Now, "Book the venue", None, TURN);
    undated.checked.completes = Some(4);
    assert_eq!(
        memory::plan(&[undated], &[], &[], &pending, &first).1,
        [Refusal::Undated]
    );
}

#[test]
fn pending_holds_at_most_five_per_session() {
    let s = session(1);
    let pending: Vec<Pending> = (1..=5)
        .map(|id| Pending {
            id,
            session: s.clone(),
            message: String::from("u-1"),
            subject: format!("subject {id}"),
            text: String::from("t"),
        })
        .collect();
    let sixth = draft(ElementKind::Now, "subject 6", None, TURN);
    assert_eq!(
        memory::plan(&[sixth], &[], &[], &pending, &s).1,
        [Refusal::PendingFull]
    );
}

#[test]
fn a_milestone_needs_a_date_and_a_turn_that_calls_it_one() {
    let s = session(1);
    let plain = "We shipped the release on november 20 at last.";
    let said = "We shipped the release on november 20, a big day for me.";
    let day = Some(date!(2026 - 11 - 20));
    assert_eq!(
        memory::plan(
            &[draft(ElementKind::Milestone, "Release shipped", day, plain)],
            &[],
            &[],
            &[],
            &s
        )
        .1,
        [Refusal::NotStated]
    );
    assert_eq!(
        memory::plan(
            &[draft(ElementKind::Milestone, "Release shipped", None, said)],
            &[],
            &[],
            &[],
            &s
        )
        .1,
        [Refusal::Undated]
    );
    let (changes, refused) = memory::plan(
        &[draft(ElementKind::Milestone, "Release shipped", day, said)],
        &[],
        &[],
        &[],
        &s,
    );
    assert!(refused.is_empty());
    assert!(
        matches!(&changes[0], PassChange::Milestone(m, _) if m.stem.as_str() == "2026-11-20-release-shipped")
    );
}

#[test]
fn an_update_rewrites_the_listed_item_in_place_and_an_unlisted_one_is_unknown() {
    let s = session(2);
    let listed = now_item(
        "2026-12-01-hackathon-deadline",
        date!(2026 - 12 - 01),
        "Hackathon deadline",
    );
    let mut moved = draft(
        ElementKind::Now,
        "Hackathon deadline moved",
        Some(date!(2026 - 12 - 03)),
        TURN,
    );
    moved.checked.update = Some(listed.stem.clone());
    let (changes, refused) = memory::plan(
        &[moved.clone()],
        std::slice::from_ref(&listed),
        &[],
        &[],
        &s,
    );
    assert!(refused.is_empty());
    let PassChange::Now(item, _) = &changes[0] else {
        panic!()
    };
    assert_eq!(item.stem, listed.stem);
    assert_eq!(item.date, date!(2026 - 12 - 03));
    assert_eq!(item.sources, [session(1), s.clone()]);
    assert_eq!(
        memory::plan(&[moved], &[], &[], &[], &s).1,
        [Refusal::Unknown]
    );
}

#[test]
fn a_now_item_leaves_fourteen_days_after_its_date() {
    let day = date!(2026 - 12 - 01);
    assert!(!memory::expired(day, date!(2026 - 12 - 15)));
    assert!(memory::expired(day, date!(2026 - 12 - 16)));
}

fn now_item(stem: &str, date: Date, title: &str) -> NowItem {
    NowItem {
        stem: stem.parse().unwrap(),
        date,
        kind: NowKind::Deadline,
        sources: vec![session(1)],
        updated: T0,
        raised: None,
        title: title.to_owned(),
        text: format!("{title}."),
    }
}

fn milestone(stem: &str, date: Date, title: &str) -> Milestone {
    Milestone {
        stem: stem.parse().unwrap(),
        date,
        sources: vec![session(1)],
        updated: T0,
        title: title.to_owned(),
        text: format!("{title}."),
    }
}

#[test]
fn the_block_prints_profile_now_nearest_first_and_the_last_five_milestones_newest_first() {
    let profile = ProfileView {
        text: profile::template(),
        as_of: Some(date!(2026 - 11 - 24)),
    };
    let now = [
        now_item(
            "2026-12-12-sisters-wedding",
            date!(2026 - 12 - 12),
            "Sister's wedding",
        ),
        now_item(
            "2026-12-01-hackathon-deadline",
            date!(2026 - 12 - 01),
            "Hackathon submission deadline",
        ),
    ];
    let milestones: Vec<Milestone> = (1..=7)
        .map(|d| {
            let day = Date::from_calendar_date(2026, time::Month::October, d).unwrap();
            milestone(&format!("2026-10-0{d}-m"), day, &format!("Milestone {d}"))
        })
        .collect();
    let block = memory::block(Some(&profile), &now, &milestones);
    let expected = format!(
        "## Profile (as of 2026-11-24)\n{}\n## Now\n\
         - 2026-12-01-hackathon-deadline · due 2026-12-01 · Hackathon submission deadline\n\
         - 2026-12-12-sisters-wedding · due 2026-12-12 · Sister's wedding\n\
         ## Milestones\n\
         - 2026-10-07-m · 2026-10-07 · Milestone 7\n\
         - 2026-10-06-m · 2026-10-06 · Milestone 6\n\
         - 2026-10-05-m · 2026-10-05 · Milestone 5\n\
         - 2026-10-04-m · 2026-10-04 · Milestone 4\n\
         - 2026-10-03-m · 2026-10-03 · Milestone 3\n",
        profile::template().trim_end()
    );
    assert_eq!(block, expected);
    assert_eq!(
        memory::block(None, &[], &[]),
        "## Profile\n(none)\n## Now\n(none)\n## Milestones\n(none)\n"
    );
}

fn known(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|s| (*s).to_owned()).collect()
}

fn with_section(heading: &str, body: &str) -> String {
    let template = profile::template();
    template.replacen(
        &format!("## {heading}\n"),
        &format!("## {heading}\n{body}\n"),
        1,
    )
}

#[test]
fn a_profile_keeps_its_headings_and_pinned_and_links_only_known_entries() {
    let tags = vec![String::from("preferences")];
    let pinned = with_section("Pinned", "- Call me Daniel.");
    let before = profile::pinned(&pinned).unwrap().to_owned();
    assert_eq!(
        profile::check(&pinned, &before, &tags, &known(&[]), &never_masks),
        Ok(())
    );
    // Pinned changed or dropped.
    assert_eq!(
        profile::check(
            &profile::template(),
            &before,
            &tags,
            &known(&[]),
            &never_masks
        ),
        Err(profile::ProfileRefusal::Pinned)
    );
    // A heading missing.
    let shape = pinned.replacen("## Projects\n", "", 1);
    assert_eq!(
        profile::check(&shape, &before, &tags, &known(&[]), &never_masks),
        Err(profile::ProfileRefusal::Shape)
    );
    let linked = with_section("Projects", "- scribe, see preferences-3.");
    assert_eq!(
        profile::check(&linked, "", &tags, &known(&[]), &never_masks),
        Err(profile::ProfileRefusal::UnknownLink)
    );
    assert_eq!(
        profile::check(&linked, "", &tags, &known(&["preferences-3"]), &never_masks),
        Ok(())
    );
    let long = with_section("Projects", &"x".repeat(profile::PROFILE_CHARS));
    assert_eq!(
        profile::check(&long, "", &tags, &known(&[]), &never_masks),
        Err(profile::ProfileRefusal::TooLong)
    );
    // The gates apply outside Pinned only.
    let money = with_section("Pinned", "- My mortgage is with the credit union.");
    let money_pinned = profile::pinned(&money).unwrap().to_owned();
    assert_eq!(
        profile::check(&money, &money_pinned, &tags, &known(&[]), &never_masks),
        Ok(())
    );
    let money_outside = with_section("Projects", "- Paying the mortgage.");
    assert_eq!(
        profile::check(&money_outside, "", &tags, &known(&[]), &never_masks),
        Err(profile::ProfileRefusal::Sensitive)
    );
}

#[test]
fn the_profile_diff_lists_removed_and_added_lines() {
    assert_eq!(profile::diff("a\nb\nc\n", "a\nc\nd\n"), "- b\n+ d\n");
    assert!(profile::diff("a\n", "a\n").is_empty());
}
