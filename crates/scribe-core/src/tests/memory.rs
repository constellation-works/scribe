use super::T0;
use crate::entry::Speaker;
use crate::memory::{
    self, Applied, Draft, MemoryConfig, MemoryId, MemoryItem, MemoryStatus, OffPage, Provenance,
    Refusal, Section,
};
use crate::models::{MemoryAnswer, PassMessage};
use crate::record::{Author, Label, Project};
use time::Duration;
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

fn element(section: &str, statement: &str, quote: &str) -> MemoryAnswer {
    MemoryAnswer {
        section: section.to_owned(),
        statement: statement.to_owned(),
        quote: quote.to_owned(),
        message: String::from("u-1"),
        ..MemoryAnswer::default()
    }
}

fn never_masks(_: &str) -> bool {
    false
}

fn check_one(
    answer: MemoryAnswer,
    turn: &str,
    config: &MemoryConfig,
) -> Result<memory::Checked, Refusal> {
    let report = memory::check(&[answer], &[user("u-1", turn)], 3, config, &never_masks);
    assert_eq!(report.used, 1);
    match (report.checked.into_iter().next(), report.refused.first()) {
        (Some(checked), None) => Ok(checked),
        (None, Some(refusal)) => Err(*refusal),
        other => panic!("one outcome expected, got {other:?}"),
    }
}

#[test]
fn a_verbatim_quote_of_his_own_turn_passes_and_anything_else_is_unverified() {
    let config = MemoryConfig::default();
    let ok = check_one(
        element("now", "Said his sister's wedding is on 2026-12-12.", QUOTE),
        TURN,
        &config,
    )
    .unwrap();
    assert_eq!(ok.section, Section::Now);
    assert!(!ok.sensitive);
    assert_eq!(
        check_one(
            element("now", "Said a wedding is soon.", "a wedding is soon"),
            TURN,
            &config
        ),
        Err(Refusal::Unverified)
    );
    // An assistant turn is never a source.
    let assistant = PassMessage {
        role: Speaker::Assistant,
        ..user("u-1", TURN)
    };
    let report = memory::check(
        &[element("now", "Said his sister's wedding is soon.", QUOTE)],
        &[assistant],
        3,
        &config,
        &never_masks,
    );
    assert_eq!(report.refused, vec![Refusal::Unverified]);
}

#[test]
fn shape_and_section_switch_refuse_before_the_write() {
    let config = MemoryConfig::default();
    assert_eq!(
        check_one(element("interests", "Likes chess.", QUOTE), TURN, &config),
        Err(Refusal::Invalid)
    );
    assert_eq!(
        check_one(element("now", &"x".repeat(201), QUOTE), TURN, &config),
        Err(Refusal::Invalid)
    );
    let mut off = MemoryConfig::default();
    off.sections.now = false;
    assert_eq!(
        check_one(
            element("now", "Said his sister marries soon.", QUOTE),
            TURN,
            &off
        ),
        Err(Refusal::SectionOff)
    );
    let disabled = MemoryConfig {
        enabled: false,
        ..MemoryConfig::default()
    };
    assert_eq!(
        check_one(element("profile", "Has a sister.", QUOTE), TURN, &disabled),
        Err(Refusal::SectionOff)
    );
}

#[test]
fn the_sensitive_gate_keeps_credentials_finance_and_standing_sensitive_facts_out() {
    let config = MemoryConfig::default();
    // Anything the redactor would change.
    let token = "my token is ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    let masks = |text: &str| text.contains("ghp_");
    let report = memory::check(
        &[element("profile", "Uses a GitHub token.", token)],
        &[user("u-1", token)],
        3,
        &config,
        &masks,
    );
    assert_eq!(report.refused, vec![Refusal::Sensitive]);
    // A listed finance word, in the statement or the quote.
    let salary = "my salary went up this year after the review";
    assert_eq!(
        check_one(element("profile", "Got a raise.", salary), salary, &config),
        Err(Refusal::Sensitive)
    );
    // A sensitive event outside now and milestones.
    let loss = "my grandmother died last week, so I'm slower";
    assert_eq!(
        check_one(
            element("working", "Works slower since his grandmother died.", loss),
            loss,
            &config
        ),
        Err(Refusal::Sensitive)
    );
    // The same event in now is kept, flagged by the word list whatever the
    // model said.
    let kept = check_one(
        element("now", "Said his grandmother died last week.", loss),
        loss,
        &config,
    )
    .unwrap();
    assert!(kept.sensitive);
    assert_eq!(kept.sensitive_kind, Some(memory::SensitiveKind::Loss));
}

#[test]
fn personal_off_refuses_flagged_and_model_marked_personal_elements() {
    let config = MemoryConfig {
        personal: false,
        ..MemoryConfig::default()
    };
    let loss = "my grandmother died last week, so I'm slower";
    assert_eq!(
        check_one(
            element("now", "Said his grandmother died.", loss),
            loss,
            &config
        ),
        Err(Refusal::PersonalOff)
    );
    let mut marked = element("now", "Said his sister's wedding is on 2026-12-12.", QUOTE);
    marked.personal = true;
    assert_eq!(check_one(marked, TURN, &config), Err(Refusal::PersonalOff));
    // Work state still passes.
    let work = "the orbit release is under way today";
    assert!(
        check_one(
            element("now", "Said an orbit release is under way.", work),
            work,
            &config
        )
        .is_ok()
    );
}

#[test]
fn a_now_statement_naming_a_mood_its_quote_does_not_is_refused() {
    let config = MemoryConfig::default();
    assert_eq!(
        check_one(
            element("now", "Is stressed about his sister's wedding.", QUOTE),
            TURN,
            &config
        ),
        Err(Refusal::Inferred)
    );
    let said = "I'm stressed about the release tomorrow";
    assert!(
        check_one(
            element("now", "Said he is stressed about the release.", said),
            said,
            &config
        )
        .is_ok()
    );
}

#[test]
fn slots_bound_the_elements_taken_and_refused_ones_use_theirs() {
    let config = MemoryConfig::default();
    let answers = vec![
        element("interests", "x", QUOTE),
        element("now", "Said his sister's wedding is on 2026-12-12.", QUOTE),
        element("profile", "Has a sister.", QUOTE),
    ];
    let report = memory::check(&answers, &[user("u-1", TURN)], 2, &config, &never_masks);
    assert_eq!(report.used, 2);
    assert_eq!(report.refused, vec![Refusal::Invalid]);
    assert_eq!(report.checked.len(), 1);
}

fn draft(statement: &str, message: &str, section: Section) -> Draft {
    Draft {
        checked: memory::Checked {
            section,
            statement: statement.to_owned(),
            quote: QUOTE.to_owned(),
            message: message.to_owned(),
            event_date: None,
            same_as: None,
            sensitive: false,
            sensitive_kind: None,
            promote: None,
        },
        event_date: None,
        turn: TURN.to_owned(),
        at: T0,
        prompt_sha256: "a".repeat(64),
        line: 1,
    }
}

fn provenance(session: &str) -> Provenance {
    Provenance {
        session: session.parse().unwrap(),
        host: Label::sanitized("host", "host"),
        project: Project::sanitized("scribe"),
    }
}

#[test]
fn a_repeat_is_an_occurrence_and_a_replayed_turn_appends_nothing() {
    let mut items: Vec<MemoryItem> = Vec::new();
    let first = draft(
        "Said his sister's wedding is on 2026-12-12.",
        "u-1",
        Section::Now,
    );
    assert_eq!(
        memory::apply(&mut items, &first, &provenance("s-1"), T0),
        Applied::Created("m-1".parse().unwrap())
    );
    assert_eq!(items[0].occurrences[0].quote, QUOTE);
    assert_eq!(items[0].occurrences[0].session.as_str(), "s-1");
    assert_eq!(items[0].author, Author::session_pass());
    // The same turn again: a replayed window.
    assert_eq!(
        memory::apply(&mut items, &first, &provenance("s-1"), T0),
        Applied::Replayed("m-1".parse().unwrap())
    );
    // Named as a repeat from a later turn: an occurrence, newest first, and
    // the user raising it marks it.
    let mut again = draft("Said the wedding is two weeks out.", "u-2", Section::Now);
    again.checked.same_as = Some("m-1".parse().unwrap());
    again.checked.statement = String::from("Said his sister's wedding is two weeks out.");
    again.at = T0 + Duration::days(7);
    assert_eq!(
        memory::apply(
            &mut items,
            &again,
            &provenance("s-2"),
            T0 + Duration::days(7)
        ),
        Applied::Appended("m-1".parse().unwrap())
    );
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].occurrences.len(), 2);
    assert_eq!(items[0].occurrences[0].n, 2);
    assert_eq!(items[0].sessions(), 2);
    assert!(items[0].acknowledged.is_some());
    // A near duplicate without same_as joins too; another section does not.
    let dup = draft(
        "Said his sister's wedding is on 2026-12-12 indeed.",
        "u-3",
        Section::Now,
    );
    assert!(matches!(
        memory::apply(&mut items, &dup, &provenance("s-3"), T0),
        Applied::Appended(_)
    ));
    let other = draft(
        "Said his sister's wedding is on 2026-12-12.",
        "u-4",
        Section::Milestones,
    );
    assert_eq!(
        memory::apply(&mut items, &other, &provenance("s-3"), T0),
        Applied::Created("m-2".parse().unwrap())
    );
}

#[test]
fn event_dates_are_kept_only_within_a_year_before_and_two_after_the_turn() {
    let turn = date!(2026 - 11 - 21);
    assert!(memory::plausible_date(date!(2026 - 12 - 12), turn));
    assert!(memory::plausible_date(date!(2025 - 11 - 21), turn));
    assert!(!memory::plausible_date(date!(2025 - 11 - 20), turn));
    assert!(!memory::plausible_date(date!(2028 - 11 - 22), turn));
}

fn item(section: Section, latest: time::OffsetDateTime) -> MemoryItem {
    let mut items = Vec::new();
    let mut d = draft("Said something worth keeping here.", "u-1", section);
    d.at = latest;
    memory::apply(&mut items, &d, &provenance("s-1"), latest);
    items.remove(0)
}

#[test]
fn decay_and_now_windows_are_derived_from_the_day() {
    let config = MemoryConfig::default();
    let day = |n: i64| (T0 + Duration::days(n)).date();
    let working = item(Section::Working, T0);
    assert_eq!(memory::off_page(&working, &config, day(90)), None);
    assert_eq!(
        memory::off_page(&working, &config, day(91)),
        Some(OffPage::Decayed)
    );
    let profile = item(Section::Profile, T0);
    assert_eq!(memory::off_page(&profile, &config, day(1000)), None);
    let undated = item(Section::Now, T0);
    assert_eq!(memory::off_page(&undated, &config, day(7)), None);
    assert_eq!(
        memory::off_page(&undated, &config, day(8)),
        Some(OffPage::PastWindow)
    );
    let mut dated = item(Section::Now, T0);
    dated.event_date = Some(day(30));
    assert_eq!(memory::off_page(&dated, &config, day(44)), None);
    assert_eq!(
        memory::off_page(&dated, &config, day(45)),
        Some(OffPage::PastWindow)
    );
    // Not yet raised until acknowledged, and again once the date passes.
    assert!(dated.not_yet_raised(day(1)));
    dated.acknowledged = Some(memory::Acknowledged {
        at: T0 + Duration::days(2),
        session: "s-1".parse().unwrap(),
        author: Author::session_pass(),
    });
    assert!(!dated.not_yet_raised(day(3)));
    assert!(dated.not_yet_raised(day(31)));
    let mut retired = item(Section::Profile, T0);
    retired.status = MemoryStatus::Retired;
    assert_eq!(
        memory::off_page(&retired, &config, day(0)),
        Some(OffPage::Retired)
    );
}

#[test]
fn the_page_keeps_its_budget_and_never_cuts_milestones() {
    let config = MemoryConfig::default();
    let mut items = Vec::new();
    for n in 0..43usize {
        let section = if n < 40 {
            Section::Working
        } else {
            Section::Milestones
        };
        let mut it = item(section, T0);
        it.id = MemoryId::numbered(u64::try_from(n).unwrap() + 1);
        it.statement = if n < 40 {
            format!(
                "Steers agent {n} with a long and specific instruction about reviews {}.",
                "x".repeat(n)
            )
        } else {
            it.event_date = Some((T0 + Duration::days(i64::try_from(n).unwrap())).date());
            format!("Milestone number {n} happened.")
        };
        items.push(it);
    }
    let recent = vec![memory::RecentSession {
        id: "s-9".parse().unwrap(),
        title: String::from("scribe memory design"),
        at: T0,
    }];
    let page = memory::page(&items, &recent, None, &config, T0.date()).unwrap();
    let milestones: usize = page
        .text
        .lines()
        .filter(|l| l.starts_with("- ") && l.contains("Milestone"))
        .map(|l| l.chars().count() + 1)
        .sum();
    assert_eq!(page.text.matches("Milestone number").count(), 3);
    assert!(
        page.text.chars().count() - milestones <= memory::PAGE_CHARS,
        "{}",
        page.text
    );
    assert!(
        page.text.contains("(+"),
        "a cut part says how many it left out"
    );
    assert!(
        page.text
            .contains("Recent sessions: s-9 \"scribe memory design\" (2026-11-01)")
    );
    assert!(!page.over_budget);
    // Nothing to show: no page.
    assert_eq!(memory::page(&[], &[], None, &config, T0.date()), None);
}

#[test]
fn forget_blanks_the_statement_and_every_quote_and_keeps_the_id() {
    let mut it = item(Section::Profile, T0);
    assert!(memory::forget(&mut it, Author::consistency(), T0));
    assert_eq!(it.status, MemoryStatus::Forgotten);
    assert!(it.statement.is_empty());
    assert!(it.occurrences.iter().all(|o| o.quote.is_empty()));
    assert_eq!(it.id.as_str(), "m-1");
    assert!(!memory::forget(&mut it, Author::consistency(), T0));
}

// Slice (b): consolidation and promotion (2_design.md §13.4, §13.5).

use crate::memory::consolidate::{self, Change, Label as PairLabel};
use crate::memory::promote::{self, PromotionRule};
use crate::models::{ConsistencyAnswer, JudgementAnswer};

/// A draft with its own statement, quote and turn.
fn said(statement: &str, quote: &str, message: &str, section: Section) -> Draft {
    let mut d = draft(statement, message, section);
    d.checked.quote = quote.to_owned();
    d.turn = format!("{quote}, as it happens.");
    d
}

fn label(pair: &str, label: &str, older: &str, newer: &str) -> ConsistencyAnswer {
    ConsistencyAnswer {
        judgements: vec![JudgementAnswer {
            pair: pair.to_owned(),
            label: label.to_owned(),
            reason: String::from("both describe where he works"),
            older_quote: older.to_owned(),
            newer_quote: newer.to_owned(),
        }],
    }
}

const HOME: &str = "I work from a home office in Boston";
const COWORK: &str = "I work from a coworking space in Boston now";

/// Two active profile items that overlap by a quarter but are not
/// duplicates: m-1 (older, session s-1) and m-2 (newer, session s-2).
fn office_pair() -> Vec<MemoryItem> {
    let mut items = Vec::new();
    let older = said(
        "Works from a home office in Boston.",
        HOME,
        "u-1",
        Section::Profile,
    );
    memory::apply(&mut items, &older, &provenance("s-1"), T0);
    let mut newer = said(
        "Works from a coworking space in Boston.",
        COWORK,
        "u-2",
        Section::Profile,
    );
    newer.at = T0 + Duration::days(30);
    memory::apply(
        &mut items,
        &newer,
        &provenance("s-2"),
        T0 + Duration::days(30),
    );
    assert_eq!(items.len(), 2, "two items, not one");
    items
}

#[test]
fn a_same_label_merges_the_newer_item_into_the_older_keeping_every_occurrence_and_quote() {
    let mut items = office_pair();
    let found = consolidate::candidates(&items, 40);
    assert_eq!(found.pairs.len(), 1);
    assert_eq!(found.pairs[0].older.id.as_str(), "m-1");
    let judged = consolidate::judge(label("1", "same", "home office", "coworking"), &found.pairs);
    assert_eq!(judged.dropped, 0);
    assert_eq!(judged.judgements[0].label, PairLabel::Same);
    let by = Author::consistency();
    let later = T0 + Duration::days(60);
    let changes = consolidate::apply(&mut items, &judged.judgements, &by, later);
    assert_eq!(
        changes,
        vec![Change::Merged {
            item: "m-2".parse().unwrap(),
            into: "m-1".parse().unwrap()
        }]
    );
    let (older, newer) = (&items[0], &items[1]);
    assert_eq!(older.status, MemoryStatus::Active);
    let quotes: Vec<&str> = older.occurrences.iter().map(|o| o.quote.as_str()).collect();
    assert_eq!(quotes, vec![COWORK, HOME], "newest first, both kept");
    assert_eq!(older.occurrences[0].n, 2, "renumbered past the target's");
    assert!(
        older
            .occurrences
            .iter()
            .all(|o| o.author == Author::session_pass()),
        "each occurrence keeps its author"
    );
    assert_eq!(older.updated_by, by);
    assert_eq!(newer.status, MemoryStatus::Merged);
    assert_eq!(newer.merged_into.as_ref().unwrap().as_str(), "m-1");
    assert!(newer.occurrences.is_empty());
    assert_eq!(
        memory::off_page(newer, &MemoryConfig::default(), later.date()),
        Some(OffPage::Merged)
    );

    // Again: nothing to pair, and the same labels change nothing.
    let before = items.clone();
    assert!(consolidate::candidates(&items, 40).pairs.is_empty());
    assert!(consolidate::apply(&mut items, &judged.judgements, &by, later).is_empty());
    assert_eq!(items, before);
}

#[test]
fn a_contradicts_label_supersedes_the_older_item_and_both_keep_their_occurrences() {
    let mut items = office_pair();
    let found = consolidate::candidates(&items, 40);
    let judged = consolidate::judge(
        label("1", "contradicts", "home office", "coworking space"),
        &found.pairs,
    );
    let by = Author::consistency();
    let changes = consolidate::apply(&mut items, &judged.judgements, &by, T0);
    assert_eq!(
        changes,
        vec![Change::Superseded {
            item: "m-1".parse().unwrap(),
            by: "m-2".parse().unwrap()
        }]
    );
    assert_eq!(items[0].status, MemoryStatus::Superseded);
    assert_eq!(items[0].superseded_by.as_ref().unwrap().as_str(), "m-2");
    assert_eq!(items[0].occurrences[0].quote, HOME);
    assert_eq!(items[1].status, MemoryStatus::Active);
    assert_eq!(items[1].occurrences[0].quote, COWORK);
    let before = items.clone();
    assert!(consolidate::candidates(&items, 40).pairs.is_empty());
    assert!(consolidate::apply(&mut items, &judged.judgements, &by, T0).is_empty());
    assert_eq!(items, before);
}

#[test]
fn a_label_survives_only_with_verbatim_quotes_and_unrelated_is_never_offered_again() {
    let mut items = office_pair();
    let found = consolidate::candidates(&items, 40);
    // A quote not in the older item's quotes, an unknown pair, an unknown
    // label: all dropped.
    for answer in [
        label("1", "same", "a home in Seattle", "coworking"),
        label("2", "same", "home office", "coworking"),
        label("1", "refines", "home office", "coworking"),
    ] {
        let judged = consolidate::judge(answer, &found.pairs);
        assert!(judged.judgements.is_empty());
        assert_eq!(judged.dropped, 1);
    }
    let judged = consolidate::judge(label("1", "unrelated", "", ""), &found.pairs);
    assert_eq!(judged.judgements[0].label, PairLabel::Unrelated);
    let by = Author::consistency();
    consolidate::apply(&mut items, &judged.judgements, &by, T0);
    assert_eq!(
        items[0].unrelated_to,
        vec!["m-2".parse::<MemoryId>().unwrap()]
    );
    assert_eq!(items[1].status, MemoryStatus::Active);
    assert!(consolidate::candidates(&items, 40).pairs.is_empty());
}

#[test]
fn a_milestones_element_with_no_rule_is_a_now_item_and_one_the_user_calls_a_big_day_is_a_milestone()
{
    let config = MemoryConfig::default();
    let mut items = Vec::new();
    let plain = said(
        "Said the scribe 1.0 release shipped.",
        "the scribe 1.0 release shipped today",
        "u-1",
        Section::Milestones,
    );
    let done = promote::write(&mut items, &plain, &provenance("s-1"), &config, T0).unwrap();
    assert_eq!(done.applied, Applied::Created("m-1".parse().unwrap()));
    assert!(done.promoted.is_empty());
    assert_eq!(items[0].section, Section::Now, "no rule held: a now item");

    // Rule (c): his turn says it is a big day.
    let mut big = said(
        "Said the orbit 2.0 launch went out.",
        "the orbit 2.0 launch went out",
        "u-2",
        Section::Now,
    );
    big.turn = String::from("the orbit 2.0 launch went out, big day for me");
    let done = promote::write(&mut items, &big, &provenance("s-1"), &config, T0).unwrap();
    assert_eq!(done.promoted.len(), 1);
    let p = &done.promoted[0];
    assert_eq!(p.rule, PromotionRule::UserSays);
    let now_item = items.iter().find(|i| i.id.as_str() == "m-2").unwrap();
    assert_eq!(now_item.status, MemoryStatus::Promoted);
    assert!(now_item.occurrences.is_empty(), "moved to the milestone");
    let milestone = items.iter().find(|i| i.id == p.to).unwrap();
    assert_eq!(milestone.section, Section::Milestones);
    assert_eq!(milestone.promoted_from.as_ref().unwrap().as_str(), "m-2");
    assert_eq!(
        milestone.promotion.as_ref().unwrap().rule,
        PromotionRule::UserSays
    );
    assert_eq!(
        milestone.occurrences[0].quote,
        "the orbit 2.0 launch went out"
    );
    assert_eq!(milestone.event_date, Some(T0.date()));

    // The same turn again changes nothing.
    let before = items.clone();
    let again = promote::write(&mut items, &big, &provenance("s-1"), &config, T0).unwrap();
    assert!(matches!(again.applied, Applied::Replayed(_)));
    assert!(again.promoted.is_empty());
    assert_eq!(items, before);

    // With now switched off, an unpromoted milestones element is refused.
    let mut off = MemoryConfig::default();
    off.sections.now = false;
    let mut none = Vec::new();
    assert_eq!(
        promote::write(&mut none, &plain, &provenance("s-1"), &off, T0),
        Err(Refusal::SectionOff)
    );
}

#[test]
fn a_now_item_raised_in_three_sessions_is_promoted_and_one_in_two_stays() {
    let config = MemoryConfig::default();
    let mut items = Vec::new();
    for (n, session) in ["s-1", "s-2"].into_iter().enumerate() {
        let mut d = said(
            "Said the box migration is under way.",
            "the box migration is under way",
            &format!("u-{n}"),
            Section::Now,
        );
        d.checked.same_as = Some("m-1".parse().unwrap());
        promote::write(&mut items, &d, &provenance(session), &config, T0).unwrap();
    }
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].status, MemoryStatus::Active, "two sessions: stays");
    let mut third = said(
        "Said the box migration is under way.",
        "the box migration is under way",
        "u-9",
        Section::Now,
    );
    third.checked.same_as = Some("m-1".parse().unwrap());
    let done = promote::write(&mut items, &third, &provenance("s-3"), &config, T0).unwrap();
    assert_eq!(done.promoted.len(), 1);
    assert_eq!(done.promoted[0].rule, PromotionRule::RecursInSessions);
    assert_eq!(items[0].status, MemoryStatus::Promoted);
    assert_eq!(items[1].section, Section::Milestones);
    assert_eq!(items[1].sessions(), 3);

    // Off at 0.
    let mut off = MemoryConfig::default();
    off.promote.recurs_in_sessions.value = 0;
    let mut kept = Vec::new();
    for (n, session) in ["s-1", "s-2", "s-3"].into_iter().enumerate() {
        let mut d = said(
            "Said the box migration is under way.",
            "the box migration is under way",
            &format!("u-{n}"),
            Section::Now,
        );
        d.checked.same_as = Some("m-1".parse().unwrap());
        promote::write(&mut kept, &d, &provenance(session), &off, T0).unwrap();
    }
    assert_eq!(kept[0].status, MemoryStatus::Active);
}

#[test]
fn a_profile_change_writes_the_milestone_and_the_new_profile_item_and_supersedes_the_old() {
    let config = MemoryConfig::default();
    let mut items = Vec::new();
    let lives = said(
        "Lives in Boston.",
        "I live in Boston these days",
        "u-1",
        Section::Profile,
    );
    promote::write(&mut items, &lives, &provenance("s-1"), &config, T0).unwrap();
    let mut moved = said(
        "Said he moved to Seattle.",
        "we finally moved to Seattle",
        "u-2",
        Section::Now,
    );
    moved.checked.promote = Some(String::from("Lives in Seattle."));
    moved.checked.same_as = Some("m-1".parse().unwrap());
    let done = promote::write(&mut items, &moved, &provenance("s-2"), &config, T0).unwrap();
    assert_eq!(done.promoted.len(), 1);
    let p = &done.promoted[0];
    assert_eq!(p.rule, PromotionRule::ProfileChange);
    assert_eq!(p.profile_from.as_ref().unwrap().as_str(), "m-1");
    let new_profile = items
        .iter()
        .find(|i| Some(&i.id) == p.profile_to.as_ref())
        .unwrap();
    assert_eq!(new_profile.statement, "Lives in Seattle.");
    assert_eq!(new_profile.section, Section::Profile);
    assert_eq!(
        new_profile.occurrences[0].quote,
        "we finally moved to Seattle"
    );
    let old = &items[0];
    assert_eq!(old.status, MemoryStatus::Superseded);
    assert_eq!(old.superseded_by.as_ref(), p.profile_to.as_ref());
    let milestone = items.iter().find(|i| i.id == p.to).unwrap();
    let journal = milestone.promotion.as_ref().unwrap();
    assert_eq!(journal.profile_from.as_ref().unwrap().as_str(), "m-1");
    assert_eq!(journal.profile_to, p.profile_to);

    // A promote statement with profile_change off promotes nothing.
    let mut off = MemoryConfig::default();
    off.promote.profile_change.value = false;
    let mut kept = Vec::new();
    let done = promote::write(&mut kept, &moved, &provenance("s-2"), &off, T0).unwrap();
    assert!(done.promoted.is_empty());
    assert_eq!(kept[0].section, Section::Now);
    assert_eq!(kept[0].status, MemoryStatus::Active);
}

#[test]
fn a_promotion_cut_short_by_a_crash_is_completed_once() {
    let config = MemoryConfig::default();
    let mut items = Vec::new();
    let mut big = said(
        "Said the orbit 2.0 launch went out.",
        "the orbit 2.0 launch went out",
        "u-1",
        Section::Now,
    );
    big.turn = String::from("the orbit 2.0 launch went out, never forget it");
    promote::write(&mut items, &big, &provenance("s-1"), &config, T0).unwrap();
    // The milestone was written; the now item's file was not.
    let milestone = items[1].clone();
    let mut crashed = vec![items[0].clone(), milestone];
    crashed[0].status = MemoryStatus::Active;
    crashed[0].promotion = None;
    crashed[0].occurrences = crashed[1].occurrences.clone();
    let by = Author::consistency();
    let changed = promote::complete(&mut crashed, &by, T0);
    assert_eq!(changed, vec!["m-1".parse::<MemoryId>().unwrap()]);
    assert_eq!(crashed[0].status, MemoryStatus::Promoted);
    assert!(
        crashed[0].occurrences.is_empty(),
        "held once, on the milestone"
    );
    assert_eq!(crashed[1].occurrences.len(), 1);
    let before = crashed.clone();
    assert!(promote::complete(&mut crashed, &by, T0).is_empty());
    assert_eq!(crashed, before);
}

#[test]
fn forget_reaches_the_items_holding_the_same_words() {
    let mut items = office_pair();
    let found = consolidate::candidates(&items, 40);
    let judged = consolidate::judge(label("1", "same", "home office", "coworking"), &found.pairs);
    consolidate::apply(&mut items, &judged.judgements, &Author::consistency(), T0);
    assert_eq!(memory::linked(&items, &"m-1".parse().unwrap()), vec![1]);
    assert!(memory::linked(&items, &"m-2".parse().unwrap()).is_empty());
}

#[test]
fn a_working_item_in_three_sessions_proposes_one_preference_and_two_propose_none() {
    let config = MemoryConfig::default();
    let mut items = Vec::new();
    let mut last = None;
    for (n, session) in ["s-1", "s-2", "s-3"].into_iter().enumerate() {
        let mut d = said(
            "Wants pull requests opened against agent-main.",
            "open the pull request against agent-main",
            &format!("u-{n}"),
            Section::Working,
        );
        if n > 0 {
            d.checked.same_as = Some("m-1".parse().unwrap());
        }
        promote::write(&mut items, &d, &provenance(session), &config, T0).unwrap();
        assert_eq!(
            memory::proposes_preference(&items[0], &config),
            n == 2,
            "{} sessions",
            n + 1
        );
        last = Some(d);
    }
    let draft = last.unwrap();
    let record = super::session(
        "0b8ef4a6-0c6a-4d77-9c41-6a4f7b2c1d10",
        T0 - Duration::hours(1),
    );
    let tags = crate::TagTable::defaults();
    let candidate = memory::preference_candidate(&items[0], &draft, &record, &tags).unwrap();
    assert_eq!(candidate.tag.as_str(), "preferences");
    assert_eq!(candidate.role, Speaker::User);
    assert_eq!(
        candidate.statement.as_deref(),
        Some("Wants pull requests opened against agent-main.")
    );
    assert_eq!(candidate.quote, draft.checked.quote);
    assert_eq!(candidate.cite.message.as_deref(), Some("u-2"));
    assert!(candidate.has_valid_provenance());
    let admitted = candidate.admitted.unwrap();
    assert_eq!(admitted.trigger, memory::RECURS_TRIGGER);
    assert_eq!(admitted.matched, "m-1");

    // Once proposed, never again; 0 switches it off; other sections never.
    let mut proposed = items[0].clone();
    proposed.preference_proposal = Some("p-1".parse().unwrap());
    assert!(!memory::proposes_preference(&proposed, &config));
    let mut off = MemoryConfig::default();
    off.promote_working_sessions.value = 0;
    assert!(!memory::proposes_preference(&items[0], &off));
    let mut now = items[0].clone();
    now.section = Section::Now;
    assert!(!memory::proposes_preference(&now, &config));
}
