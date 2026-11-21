use crate::issue_trigger::fire;
use crate::tag::CaptureRules;

#[test]
fn the_trigger_fires_only_in_daniels_own_words_under_the_agent_route() {
    let rules = CaptureRules::shipped("issue");
    let hit = fire(&rules, "Can you investigate why CI is red?").unwrap();
    assert_eq!(hit.trigger, "can you investigate");
    assert_eq!(hit.matched, "Can you investigate");
    for prompt in [
        "#scribe preferences: always find the root cause",
        "> root cause: the cache",
        "    root cause: the cache",
        "```\nroot cause\n```",
        "<pasted_content>root cause</pasted_content>",
        "<task-notification>root cause</task-notification>",
        "/scribe on",
        "add a toggle",
    ] {
        assert_eq!(fire(&rules, prompt), None, "{prompt}");
    }
    // Under another route the hook takes no trigger.
    assert_eq!(
        fire(&CaptureRules::shipped("preferences"), "I prefer tabs"),
        None
    );
}
