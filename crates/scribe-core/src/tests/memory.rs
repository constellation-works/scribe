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
        },
        event_date: None,
        turn: TURN.to_owned(),
        at: T0,
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
