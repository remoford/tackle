# tackle

A desktop harness for running a fleet of Claude Code sessions that each carry a very
large, verbatim working context, with context lifecycle managed strictly by hand.

## The problem

The work is intricate mathematics: a ~700k-token corpus of conjectures and theorems that
several sessions work on together. Every session, including the orchestrator, needs the
whole corpus in context, word for word.

- **No compaction, ever.** A model-written summary of mathematics silently loosens
  statements and drops hypotheses. A compacted context is contaminated.
- **No journals or deltas.** Carrying an old and a revised version of a result in one
  context poisons reasoning. When the corpus changes, a session is cleared and re-reads
  the current corpus from scratch.
- **Sessions can't `/clear` or `/context` themselves.** Something outside them has to.
- **Every session must stay reachable over Remote Control.**

## The shape

- **tackle** (this repo): a small GUI app, Rust + egui like `../diet/app`. It hosts each
  Claude session as an interactive `claude --remote-control <name>` inside a ConPTY, so it
  can type `/context` and `/clear` into them and read (scrape) what they show. It shows
  what each session is doing, its context fill over time, the traffic between sessions,
  cache hit rates, and cost.
- **Workers and orchestrator**: Opus sessions started in the corpus project, whose
  `CLAUDE.md` `@`-imports the corpus files. `/clear` reloads the imports from disk, so a
  clear *is* the re-read.
- **Names**: every session tackle starts is named `tk-...` (hr is `tk-hr`, workers
  `tk-ic-1`, ...), so tackle's sessions stand out in `ListAgents` and Remote Control.
- **hr**: the one special session, on Haiku to be as cheap as possible. tackle starts it
  at launch in its own directory (`%LOCALAPPDATA%\tackle\hr`, no corpus) and restarts it
  whenever it exits, so it is always alive. It relays between the other sessions and
  tackle (context, clear, session state) and carries no mathematics. Its state lives in
  tackle, not in its context, so it can be cleared freely. hr is effectively tackle's API
  over plain language: it alone gets tackle's MCP server (`tackle.exe --mcp`, a stdio pipe
  to the running app) with tools to list sessions and projects, read a session's context
  usage or its exact screen, queue `/clear` or `/context` for its next idle moment, type
  into a session, start and stop sessions, and hand out the writer token. tackle writes
  hr's `CLAUDE.md` (from `src/hr.md`) on every launch.
- **Authority**: looking is open to all; acting on a session (typing, keys, clear, stop,
  restart) only to the human, delegates the human names in tackle (keyed by the
  messaging address, revocable), the session itself, and the sessions above it in the
  chain that started it. hr passes each request's harness-set `from` as `requested_by`.
  Every act and refusal goes to `%LOCALAPPDATA%	acklections.log`.
- **Orchestration files**: the book says what the work is; the project's gitignored
  `orchestration/` says who is doing it. Each session keeps
  `orchestration/state/<name>.md`, whose last line `BETWEEN UNITS <commit>` marks a
  handover. The format is in `docs/orchestration.md`.
- **`tk` CLI** (`src/bin/tk.rs`): every MCP tool as a command, over the same port
  (written to `%LOCALAPPDATA%	ackle\port`). tackle finds the calling process from the TCP
  table and walks its parents: under a tk session it has that session's rights, else the
  human's. Use it for debugging instead of going through hr, and don't spin up test
  sessions needlessly: the user is wary of account limits.
- **Plan usage**: tackle types `/usage` into hr every 10 minutes while hr is idle (a local
  command, no tokens), parses the limits and resets, stores them in `usage.jsonl` with
  per-session token totals, and plots them with a projection.
- **Persistence**: `%LOCALAPPDATA%	ackleleet.json` holds settings, delegates, writer
  tokens and every session's record (tackle id, stable across clears; name, role,
  project, manager, Claude session id, read commit). After a restart of tackle they are
  listed as not running, to resume by id or start fresh.
- **Projects**: named directories, added by hand in tackle's Projects panel and kept in
  `%LOCALAPPDATA%\tackle\projects.json`; there is no discovery. "Start a worker for ic"
  starts `tk-ic-1` in the ic project's directory.
- **Engine**: fleet state, hooks, hr's tools and the rules run on a background thread, not
  in the GUI's frame loop, because a window hidden in the tray stops repainting.
- Sessions talk to each other directly with `SendMessage` by name. Math never passes
  through hr.

## Rules tackle enforces mechanically

Rules 1 and 2 were revised on 2026-09-30 after review by the session that ran the old
workflow (ic-7d): a full read costs ~680k-800k tokens and 8-9 minutes, and "between
turns" is not "between tasks".

1. **Clear only at a handover.** A session is cleared when its manager or the human asks,
   or automatically once its state file ends in `BETWEEN UNITS` and its context has
   passed the "clear at handover" size. hr, which holds no state, clears on size alone.
2. **Show staleness; the manager decides.** tackle records the commit each session read
   and shows "stale by N" commits it did not make itself. It does not clear on corpus
   changes.
3. **No compaction, ever.** Sessions run with `DISABLE_AUTO_COMPACT` and `DISABLE_COMPACT`,
   and the `PreCompact` hook blocks compaction (exit 2). A session that compacts anyway is
   stopped and may only be started fresh from its state file, never resumed.
4. **One writer, one build.** In a project marked "one writer", only the writer-token
   holder may edit or `git commit`/`git push` (a `PreToolUse` hook refuses the rest). One
   Bash command containing a build-lock word (`lake`, `xelatex`, `latexmk`) at a time
   across the fleet; others are refused at once, never queued.

## What has been verified (2026-09-30)

- `/context` and `/clear` work over headless `stream-json`; `/context` also yields a
  structured `context_usage` object, and the transcript gets a `local_command` entry with
  `contextUsage`.
- `CLAUDE.md` `@`-imports are re-read from disk on `/clear` (tested at 17k tokens, not 700k).
- Headless sessions started with `-n <name>` are listed by `ListAgents`, wake on an
  incoming `SendMessage`, and reply; `notify_when_idle` works on them.
- `-p` plus `--remote-control` does not start a session, which is why tackle uses ConPTY.
- An interactive `claude --remote-control <name>` runs inside a ConPTY and connects.
- A session tackle hosts (hr, with `CLAUDE_CODE_CHILD_SESSION` and friends stripped from
  its environment) is listed by `ListAgents` as interactive and Remote Control, receives
  `SendMessage`, and replies by name. Its `SessionStart` hook reaches tackle.

- Background jobs: Claude's `run_in_background` shell detaches, so the job's processes
  are orphans, not descendants of `claude.exe`. tackle finds them by diffing the process
  list between the call's `PreToolUse` and `PostToolUse`; the build lock and the "turn
  ended, background work running" state follow those pids (tested with `sleep`).
- The writer token refuses `Write` without the token and allows it after a grant; the
  build lock refuses a second lock-word command; resume by Claude session id works
  after a tackle restart (all tested on a scratch repo with a Haiku worker).
- Prompt caching across separately launched sessions (measured 2026-09-30 from eight
  fresh tk-hr sessions): the tools + system prompt prefix (33,275 tokens) is read from
  cache by every new session, even 22 minutes apart. CLAUDE.md is **not** shared: it
  arrives in the first message, after the cached prefix, and was written to cache anew by
  every session even when identical. So each fresh session or `/clear` pays the full cache
  write for a CLAUDE.md-imported corpus; staggering clears saves nothing.
- Costs: transcripts record each message's model and splits cache writes into 1-hour
  (2x input) and 5-minute (1.25x) parts; `src/pricing.rs` prices them at API list rates.
  A session's share of the weekly limit is estimated by splitting each rise in "Current
  week (all models)" by what sessions spent; spending tackle can't see is "outside tackle".
- Auto-compaction: this machine's user settings, `~/.claude/settings.json`, have
  `"autoCompactEnabled": false`. The Claude Code binary also honours `DISABLE_AUTO_COMPACT` and
  `DISABLE_COMPACT` (the latter also refuses `/compact`), and a `PreCompact` hook exiting
  2 blocks compaction ("continuing uncompacted").

## Open questions

- Do typed `/context` and `/clear` behave in a ConPTY session the same as over stream-json?
- Could the corpus be shared through `--append-system-prompt-file` instead? The appended
  text becomes part of the system prompt, which sessions do share, and Claude Code records
  it at session start and re-sends it unchanged (so `/clear` would not re-read it; a
  corpus change would mean a fresh session). Untested; one cheap test with two small
  sessions would settle it. Don't run it without the user's go-ahead.
- Can tackle tell when a session holds an incoming cross-session message for approval?
  It shows notifications and permission prompts; a held message may not raise either.
