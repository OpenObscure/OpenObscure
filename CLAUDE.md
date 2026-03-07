# OpenObscure — Claude Code Instructions

Development conventions (feature gating, test commands, project structure) are in [CONTRIBUTING.md](CONTRIBUTING.md).

## Guardrails

- **Feature gating is mandatory** — follow the checklist in CONTRIBUTING.md for every new feature
- **Never modify code in the test repo** (`/Users/admin/Test/OpenObscure`) — commit, push, pull there
- **Never commit `project-plan/`** to git — it's in `.gitignore`
- **Never copy code or binaries from dev to test env** — commit → push, pull in test env, build there
- **Enterprise-only features** (compliance CLI) must NOT appear in public-facing docs (README, ARCHITECTURE, setup/, integration/)

## Session Notes

- Create at every `/compact` point and end of session
- Format: `session-notes/ses_YY-MM-DD-HH-MM.md`
- Phase plans: `project-plan/PHASE<N>_PLAN.md` (gitignored)
