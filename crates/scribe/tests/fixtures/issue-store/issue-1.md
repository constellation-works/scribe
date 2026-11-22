---
format: 23
kind: issue
id: issue-1
title: GitHub API secondary rate limit during drains
status: open
signature:
- HTTP 429
projects:
- orbit
hosts:
- host-a
topics:
- github
related:
- task:ORB-1
- session:7e2d4c1a-9b3f-4e8d-a6c5-1f0e2d3c4b5a
author:
  kind: agent
  name: opus
updated_by:
  kind: agent
  name: opus
created: 2026-11-22T04:09:43Z
updated: 2026-11-22T04:09:43Z
occurrences:
- n: 2
  date: 2026-11-22
  host: host-a
  project: orbit
  session: 7e2d4c1a-9b3f-4e8d-a6c5-1f0e2d3c4b5a
  request_message: u-2
  recurred: false
  moved_from: null
  author:
    kind: agent
    name: opus
- n: 1
  date: 2026-11-22
  host: host-a
  project: orbit
  session: 7e2d4c1a-9b3f-4e8d-a6c5-1f0e2d3c4b5a
  request_message: u-1
  recurred: false
  moved_from: null
  author:
    kind: agent
    name: opus
gone: []
---
## Cause
cause not found
## Fix
fix not found
## Occurrences
### 2. 2026-11-22 — host-a, orbit
Hit the limit again.
**The user asked:** "the drain failed again, find out why it keeps hitting the limit"
**Error lines (agent evidence):**
**Cause:** Pushes from parallel drains share one token.
**Fix:** Serialise pushes per token.
**Links:** session:7e2d4c1a-9b3f-4e8d-a6c5-1f0e2d3c4b5a
### 1. 2026-11-22 — host-a, orbit
The drain stopped after the GitHub API refused pushes.
**The user asked:** "can you investigate why the drain keeps failing with HTTP 429"
**Error lines (agent evidence):**
    HTTP 429: You have exceeded a secondary rate limit
**Cause:** cause not found
**Fix:** fix not found
**Links:** session:7e2d4c1a-9b3f-4e8d-a6c5-1f0e2d3c4b5a, task:ORB-1
