# Machine checks — STD-14 Pre-release legacy support

The gate that enforces each rule in [STD-14](STD-14.md), or `review-only`. Ship a change with the gate its rule names, or say it is review-only.

| Rule | Gate |
|---|---|
| R1 | review-only (when `git tag` has no release tag, a reviewer flags any retained superseded-format reader, importer, migration or compatibility shim) |
| R2 | review-only |
