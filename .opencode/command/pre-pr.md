---
description: Run Stalagmight's adversarial pre-PR review gate
agent: build
---
Run the pre-PR review exactly as defined in `.agents/skills/pre-pr/SKILL.md`, with the reviewer
briefs in `.agents/skills/pre-pr/references/reviewers.md`. Read both files completely first; they
are the single source, shared with Claude Code's `/pre-pr` and Codex's `$pre-pr`.

Where the skill says "Codex collaboration subagents", launch the reviewers concurrently with the
Task tool as `general` subagents, one per reviewer, each given only its brief. Never run the
review in this session's own context.
