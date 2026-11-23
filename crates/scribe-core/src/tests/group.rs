use crate::SessionId;
use crate::group::automatic_seat;

#[test]
fn automatic_seat_stops_on_first_lookup_error() {
    let session: SessionId = "abcdefgh-one".parse().unwrap();

    let mut probed = Vec::new();
    let err = automatic_seat(&session, |name| {
        probed.push(name.to_owned());
        Err::<bool, _>("lookup failed")
    })
    .unwrap_err();
    assert_eq!(err, "lookup failed");
    assert_eq!(probed, ["abcdefgh"]);

    // Also stops promptly if a later probe (full id or numeric suffix) fails.
    let mut probed = Vec::new();
    let err = automatic_seat(&session, |name| {
        probed.push(name.to_owned());
        if name == "abcdefgh" {
            Ok(true)
        } else {
            Err("second lookup failed")
        }
    })
    .unwrap_err();
    assert_eq!(err, "second lookup failed");
    assert_eq!(probed, ["abcdefgh", "abcdefgh-one"]);
}

#[test]
fn automatic_seat_walks_short_full_and_numbered_candidates() {
    let session: SessionId = "abcdefgh-one".parse().unwrap();
    let taken = ["abcdefgh", "abcdefgh-one", "abcdefgh-one-2"];
    let mut probed = Vec::new();
    let seat = automatic_seat(&session, |name| {
        probed.push(name.to_owned());
        Ok::<_, std::convert::Infallible>(taken.contains(&name))
    })
    .unwrap();
    assert_eq!(seat.as_str(), "abcdefgh-one-3");
    assert_eq!(
        probed,
        [
            "abcdefgh",
            "abcdefgh-one",
            "abcdefgh-one-2",
            "abcdefgh-one-3"
        ]
    );
}

#[test]
fn automatic_seat_bounds_long_numbered_candidate_stem() {
    use crate::Label;

    let id = "a".repeat(128);
    let session: SessionId = id.parse().unwrap();
    let short = "a".repeat(8);
    let numbered = format!("{}-2", "a".repeat(Label::MAX_CHARS - 12));
    let mut probed = Vec::new();

    let seat = automatic_seat(&session, |name| {
        probed.push(name.to_owned());
        Ok::<_, std::convert::Infallible>(name == short || name == session.as_str())
    })
    .unwrap();

    assert_eq!(seat.as_str(), numbered);
    assert_eq!(probed, [short, id, numbered]);
}
