# Initialize a project

On the human's request, scaffold the skill and hooks in the explicit project target.

```sh
scribe init --provider codex --target <project-directory>
scribe init --provider claude-code --target <project-directory>
```

The binary writes this skill with its references beside `SKILL.md` and
scaffolds provider hooks. It preserves
unrelated configuration and refuses conflicting edits. Codex hooks require
review and trust in **Settings > Hooks** (desktop) or `/hooks` (CLI), then a
restart/resume. Initialization and loading this skill do not enable recording.
