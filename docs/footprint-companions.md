# Footprint companions

Tasks keep declaring one half of a coupled set and then widening their
footprint at commit time. Declare the whole set up front: each row below
names the canonical side, the derived or mirrored side, the regeneration
command or sync rule that links them, and when the companion changes.
`scripts/check-footprint-companions.sh` (wired into `make structure`)
checks that every path in the Checked entries block exists and that every
named regeneration command is a `Makefile` target. To add a set here, cite
its generator or sync rule; sets without one do not belong.

| Set | Canonical side | Derived / mirrored side | Regeneration command or sync rule | When the companion changes |
| --- | --- | --- | --- | --- |
| Architecture contract set | [the architecture spec](design/scribe/specs/architecture.md) | [ARCHITECTURE.md](../ARCHITECTURE.md), [the dependency-direction gate](../scripts/check-dependency-direction.sh), [the gate cases](../scripts/tests/check-gates.sh), and [2_design.md](design/scribe/2_design.md) where it carries the changed claim | Same-commit rule in [ARCHITECTURE.md](../ARCHITECTURE.md): a new crate, edge, app or guard changes that file, the spec and those scripts in the same commit; the direction gate cites the spec's Guards section as its source | A new crate, edge, app, guard, layer order or root place; any claim the spec and the map both state (drift here has regressed before: [ORB-28910] fixed the claim [ORB-28890] broke, [ORB-28884] fixed the claim [ORB-28860] broke, [ORB-28827] fixed the claim [ORB-28614] broke) |
| Generated goldens | The surface sources: help text, output shaping (`crates/scribe/src/output/`), the fixture store, and [the golden harness](../crates/scribe/tests/goldens.rs) | The golden files under [crates/scribe/tests/goldens/](../crates/scribe/tests/goldens/) (`help/*.txt`, `output/*`) | `make goldens UPDATE=1` rewrites the goldens instead of comparing (see [Makefile](../Makefile) `goldens` target and Gates in [AGENTS.md](../AGENTS.md)); [goldens.rs](../crates/scribe/tests/goldens.rs) is the harness that compares them | After any intended `--help` or output change; review the regenerated diff as a contract change before review |
| Shared test support | The integration test that needs a new fixture helper | [tests/support/mod.rs](../crates/scribe/tests/support/mod.rs), imported by the integration tests with `mod support;` | Co-change rule in [AGENTS.md](../AGENTS.md) Tests: repository fixtures live in `tests/support`, so extend the helper and its calling test in the same commit | A test needs a fixture helper (`sql`, `snapshot`, `set_switch`, `exported`, or a new one); touching only the caller leaves the next author to widen the footprint |

## Checked entries

The structure gate reads exactly these lines: one `- path:` line per
checked path, one `- make-target:` line per regeneration command. Keep
each entry on its own line in this form so the gate and the table cannot
drift apart.

- path: `ARCHITECTURE.md`
- path: `docs/design/scribe/specs/architecture.md`
- path: `docs/design/scribe/2_design.md`
- path: `scripts/check-dependency-direction.sh`
- path: `scripts/tests/check-gates.sh`
- path: `crates/scribe/tests/goldens.rs`
- path: `crates/scribe/tests/goldens/`
- path: `crates/scribe/tests/support/mod.rs`
- make-target: `goldens`

## Task References

- [ORB-29107] filed this map and its structure check.
