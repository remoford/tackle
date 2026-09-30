# Orchestration files

How a session run by tackle records who it is and where it has got to, so that a
replacement can pick up after a clear, a restart or a contamination, and so that tackle
(or a manager) can see progress without waking the session. The format is plain text and
works without tackle: a session that isn't hosted by tackle can keep it the same way.

The split, agreed with the sessions that ran the old workflow:

- **The book says what the work is.** The plan, its units and their done-when clauses live
  in the project's committed files (for ic: `book/roadmap/programme.tex` and each part's
  roadmap). Nothing here copies them.
- **`orchestration/` says who is doing it.** Live assignments, handovers and progress live
  in the project's gitignored `orchestration/` directory. tackle reads and writes only
  this, never the book.

## The state file

One per session: `orchestration/state/<session name>.md` in the project directory (tackle
sets `TACKLE_STATE_FILE` to its path). The session writes it; tackle only reads it. Keep
it short: every replacement reads it right after a full read of the corpus.

```
session: tk-ic-1   tackle id: <TACKLE_ID>
role: worker   manager: tk-ic-orch
read: book/main.tex and part II (chapters 4-6), done 14:02, 781k tokens
unit: <one line: which unit, from the book>
context: 912k (last known)

rulings not yet in CLAUDE.md (word for word, dated):
- 2026-09-30 author: "reports go under orchestration/reports/"

open questions (to whom, when, answered?):
- to author, 14:20: does step 3 need the full audit? (open)

uncommitted files: src/Foo.lean
last commit produced: <hash>
reports: orchestration/reports/unit1.md (84 lines)
departures disclosed: commit message written with a heredoc (told manager 15:00)

status (appended, newest last):
14:02 read done
14:40 running lake build
15:05 BETWEEN UNITS <commit>
```

Rules:

- **Status lines are appended, never rewritten**, with the time and the tool or phase
  named ("running lake build", "running validator", "waiting on author").
- **`BETWEEN UNITS <commit>` as the last line is a handover**: the unit is finished,
  committed and pushed, and everything above is current. Only then may tackle clear the
  session on its own (see "Clearing" below). Starting new work means appending a new
  status line, which ends the handover.
- **Rulings, open questions, uncommitted files and reports** are what a replacement needs
  and can't get from the book. Keep them current.

## Clearing and restarting

- A session is cleared (`/clear`, which re-reads the corpus from disk) only when its
  manager or the human asks, or automatically when its state file ends in `BETWEEN UNITS`
  and its context has passed the "clear at handover" size set in tackle.
- A clear costs a full read (about 680k–800k tokens and 8–9 minutes on the ic corpus), so
  reuse a session while it has room.
- **Staleness**: tackle records every file a session reads with the Read tool, with the
  file's modification time. Once a file's modification time is newer, it is stale for
  that session (its own edits don't count). tackle shows how many read files changed and
  which. It does not clear on corpus changes: the manager batches them and decides.
- A `/clear` drops everything the session has read; it then reads what its next task
  needs. The state file should say what that is.
- **Compaction is never allowed.** Sessions run with auto-compaction and `/compact`
  disabled, and a hook refuses compaction. A session that compacts anyway is stopped and
  may only come back as a fresh session told to resume from its state file, never resumed
  by id.
- **Resume by id** (`claude --resume`) is fine for a clean session that is merely stopped.

## The writer token and the build lock

The author forbids branches and worktrees, so sessions share one checkout.

- In a project marked "one writer" in tackle, only the session holding the writer token
  may edit files or run `git commit` / `git push`; tackle refuses those calls from anyone
  else. The manager grants the token; the holder pulls before editing, and releases the
  token after committing and pushing.
- Only one heavy build runs at a time across the fleet: a Bash command containing a
  build-lock word (by default `lake`, `xelatex`, `latexmk`) is refused while another holds
  the lock, with a message saying who holds it. Wait and retry; nothing queues.

## Authority

Only the human, delegates the human names in tackle, the session itself, and the sessions
above it in the chain that started it may type into a session, press its keys, clear, stop
or restart it (through tk-hr). Every such act is logged with who asked, in tackle's
`actions.log`.
