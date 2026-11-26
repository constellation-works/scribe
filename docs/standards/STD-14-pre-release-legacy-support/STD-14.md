---
id: STD-14-pre-release-legacy-support
title: Pre-release legacy support — no compatibility code before a project's first release tag
summary: Normative rules for unreleased projects (no release tag in the repository): delete superseded format and layout code instead of keeping readers, importers, migrations or compatibility shims until a release exists; follow in any application or tool.
tags: [standard, releases, legacy, compatibility, stateful]
version: 1
created_date: 2026-11-26
updated_date: 2026-11-26
last_validated: 2026-11-26
related: [STD-02-rust-architecture-and-errors, STD-03-concurrency-and-process-safety, STD-01-cli-surface, STD-09-project-docs]
---

# Pre-release legacy support

An application or tool that has never published a release has no users to support. Keeping readers, importers, migrations or compatibility shims before the first release tag adds dead weight and maintenance cost for callers that do not exist; legacy-support duties begin once a release tag is published.

This standard is a directory. This file holds the binding text: the rules, where they apply, and how to deviate. Beside it, [`why.md`](why.md) gives the reason for each rule, [`checks.md`](checks.md) the gate that enforces it, [`exemplars.md`](exemplars.md) worked examples in Orbit, and [`CHANGELOG.md`](CHANGELOG.md) what changed in each version.

## Rules

- **R1.** *No legacy support before the first release.* An application or tool whose repository has never published a release (no release tag in its repository) MUST NOT keep legacy support — no reader, importer, migration or compatibility shim — for a format or layout it has superseded. When a format or layout changes before the first release tag, the superseded code, tests and fixtures MUST be deleted rather than kept or moved into a legacy module.
- **R2.** *Legacy duties start with the first release tag.* The obligations to retain, quarantine and migrate superseded formats, schemas or interfaces (`STD-02 §R16`, the compatibility exception of `STD-02 §R17`, `STD-02 §R44`, `STD-03 §R23`–`§R24`, and `STD-01 §R35`) begin with the repository's first release tag; before that tag, live code MUST read and write only the current shape.

## Applies to

- `universal`: R1 and R2 bind any application, tool or library repository that persists data, parses stored state, or changes an external or on-disk format.
- `stateful`: applies alongside STD-02 §R16, §R17 and §R44 and STD-03 §R23–§R24; those legacy-retention and migration duties take effect once the repository has its first release tag.

## Deviations

An adopting repo that departs from a rule records a decision in its own design docs, citing `STD-nn@<version> §Rn` and the reason. It never edits its vendored copy of this standard.
