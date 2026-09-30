# tackle

A Windows desktop app that runs a fleet of Claude Code sessions, keeps each one reachable
over Remote Control, and manages their context by hand.

> [!CAUTION]
> **tackle is distinctly unsafe and has minimal safeguards. Use it at your own risk. I
> don't recommend using it.**
>
> - It runs many Claude Code sessions unattended, each able to run commands, edit files
>   and commit in your repositories, and each costing real money or plan usage.
> - It lets sessions act on each other: one session can ask tk-hr, tackle's relay
>   session, to type prompts or press keys in another. The checks on who may do that are
>   thin. They trust names and addresses reported by the messaging system and the relay
>   model's honesty about who asked, and they fail open when tackle isn't running.
> - It types into terminals and reads their screens. A misread screen or a keystroke at
>   the wrong moment can answer a prompt you didn't mean to answer.
> - Its locks (one writer per checkout, one build at a time) are advisory hooks. They
>   only cover sessions tackle started, and a session can get around them.
> - The `tk` command line has your rights unless tackle can trace the calling process
>   back to one of its sessions. Anything else on your machine that runs `tk` acts as
>   you.
> - It is a personal tool built for one workflow, tested lightly, on one machine.
>
> If you use it anyway, use it only on repositories you can afford to lose, with a
> spending limit you can afford to hit.

## What it is for

It was built for work on a large mathematical corpus (about 700k tokens) that several
sessions need in context word for word. That brings rules a normal harness doesn't have:

- **No compaction, ever.** A summarised proof is a broken proof. Sessions run with
  compaction disabled, a hook refuses it, and a session that compacts anyway is stopped.
- **Clear only at a handover.** A full re-read costs hundreds of thousands of tokens, so
  a session is cleared only when its manager asks, or once it has written "between units"
  in its state file.
- **Show staleness; let the manager decide.** tackle records every file each session
  has read and shows which of them have changed since.

## What it does

- Hosts each session as an interactive `claude --remote-control <name>` inside a ConPTY,
  with a built-in terminal view, so every session is also reachable from your phone or
  claude.ai. Sessions are named `tk-...`.
- Shows each session's state (busy, idle, needs input, waiting on its own background
  build), context size over time, cache hit rate, the tool it is running, which of the
  files it has read have changed since, and whether its cache has likely gone cold.
- Feeds all of that from Claude Code hooks and transcripts, not screen scraping.
- Runs **tk-hr**, an always-on Haiku session that is tackle's interface in plain language:
  other sessions (or you, over Remote Control) ask it who is running, what a session is
  doing, for a session's exact screen, to clear or start a session, and so on.
- Reads `/usage` on tk-hr every few minutes and plots plan usage, which sessions used it,
  and when each limit runs out at the current rate.
- Prices every session at API list rates, and estimates each one's share of the weekly
  limit and of the monthly plan price (e.g. $200 for Max 20x). The estimate splits each
  rise in the weekly percentage by what tackle's sessions spent; usage from sessions
  outside tackle shows as "outside tackle".
- Remembers sessions, settings, delegates and locks across restarts, and can resume a
  session by id or start it fresh from its state file.
- Alerts you with a Windows notification when a session needs input, is holding a
  message from another session for approval, compacts, or exits on its own.
- Logs the messages its sessions send each other, keeps a per-project file of who holds
  which unit of work, and saves presets for starting sessions.
- Minimises to the system tray, optionally starts with Windows, and closing the window
  keeps the sessions running.

## Requirements

- Windows 10 or 11.
- [Claude Code](https://code.claude.com) installed and signed in (`claude` on `PATH`, or
  set `TACKLE_CLAUDE` to `claude.exe`).
- Rust (stable) to build.

## Build and run

```
cargo build --release
target\release\tackle.exe
```

tackle keeps its state in `%LOCALAPPDATA%\tackle`: `fleet.json` (sessions, settings,
delegates, locks), `projects.json`, `actions.log` (every act and refusal, with who asked),
`usage.jsonl`, and tk-hr's working directory.

## Command line

`tk.exe`, built alongside tackle, exposes every tool tk-hr has:

```
tk help                                  list commands
tk help screen                           one command's arguments
tk list-sessions
tk activity tk-ic-1
tk screen tk-ic-1 --lines-back 40
tk send-keys tk-ic-1 down enter
tk start-session ic --model sonnet --brief "..."
tk usage
tk set-settings --hr-clear-at 150000
```

Required arguments go in order; the rest as `--name value`. tackle decides who is
calling from the process tree: from inside a tackle session, `tk` has that session's
rights; from anywhere else, yours.

## Documentation

- [`docs/orchestration.md`](docs/orchestration.md): the working rules for sessions under
  tackle (state file, handovers, reading rule, locks, authority). Agents don't need to
  find it: tk-hr's `protocol` tool and `tk protocol` hand out its sections word for word.
- [`CLAUDE.md`](CLAUDE.md): design notes, the rules tackle enforces, and what has and
  hasn't been verified.

## License

MIT; see [LICENSE](LICENSE).
