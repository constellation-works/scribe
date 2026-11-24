use crate::Stem;
use time::macros::date;

#[test]
fn a_stem_is_the_date_and_a_slug_of_the_title() {
    assert_eq!(
        Stem::new(date!(2026 - 12 - 12), "Sister's wedding!").as_str(),
        "2026-12-12-sister-s-wedding"
    );
    assert_eq!(
        Stem::new(date!(2026 - 12 - 12), "!!").as_str(),
        "2026-12-12-item"
    );
    assert!("2026-12-12-Sister".parse::<Stem>().is_err());
    assert!("../2026-12-12-x".parse::<Stem>().is_err());
}
