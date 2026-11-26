# Why — STD-14 Pre-release legacy support

Why each rule in [STD-14](STD-14.md) exists, tied to the incident behind it where there was one. Read the entry for a rule when its intent is unclear or before deviating. The rules themselves are in `STD-14.md`; this file explains them and adds none.

- **R1 (no legacy support before first release).** Without a published release there are no external users or deployed stores from an earlier release to break. Keeping legacy readers and importers before release forces unreleased codebases to maintain compatibility paths for formats that never shipped; motivated by ORB-28767, where unreleased `scribe` kept legacy store readers under `STD-02 §R44` (scribe standard-24).
- **R2 (legacy duties begin at first release).** Rules that require backward-compatible readers and quarantined upgrade units assume released versions exist in the wild. Pinning their start to the first release tag reconciles `STD-02 §R44` and `§R17` with unreleased development while keeping every released repository under their full protection (scribe standard-24, ORB-28767).
