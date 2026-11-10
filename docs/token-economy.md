# Token economy (every agent and scheduled task follows this)

Origin: token audit 2026-11-09. Short on purpose; the rules, not the reasons.

- **Concise output.** Lead with the result; no recap of what you just did.
- **Grep before read; read ranges, not whole files.** Don't re-read a file after you edited it.
- **Batch independent tool calls in one message.** One script that prints everything beats five calls.
- **Summarise logs; never paste them.** Quote the failing lines only.
- **Peer messages: at most 5 lines.** No re-sent context; milestones only.
- **No images unless a decision needs one.** Scale or crop when you must.
- **Search and reading go to Sonnet or Haiku subagents;** keep Opus for design and judgement.
- **Subagents stop at about 100k tokens or 30 minutes** and report what is done and stuck.
- **Rote or token-free work goes local:** tag the issue `compute:local-llm` / `compute:cpu` (`docs/local-compute.md`).
- **Check context** (`get_usage`); at about 45% write or refresh a state note (queue, in flight, waiting on), then keep going: auto-compaction handles the rest, and you re-read the note after each compaction.
- **Scheduled tasks:** skip silently when nothing changed; cap tool calls in the prompt.
