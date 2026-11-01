# aio-rs for coding agents

`ax agent` is the brief an agent reads before it touches an aio-rs app:
the rules that protect the person first, then how aio-rs works, then every
verb. It is made from the code (the verb table, the brief's sections), so
it matches the `ax` installed.

```sh
ax agent              # the page
ax agent --min        # compact forms
ax agent --max        # with the deep sections (api, cli, pitfalls)
ax agent --list       # the sections
ax agent ui           # one section in full
ax agent --json       # the same, addressable
```

The sections: `rules`, `model`, `test`, `debug`, `ship`, `new`, `cell`,
`ui`, `sync`, `data`, `api`, `cli`, `pitfalls` (`ax agent --list` says
which are deep, shown only with `--max` or by name).

Every app `ax create` makes has an `AGENTS.md` (and a `CLAUDE.md` that
imports it): the rules and the loop below, inline, then the brief and this
guide by URL (no path of the machine it was made on).

## The rules

1. Never kill by process match (`pkill`, `killall`): `ax stop <app>`.
2. Never take over the person's screen: an agent's windows go on a nested
   display (`ax dev` does it by itself when it has no terminal);
   `ax build --smoke` and UI tests run headless.
3. Never script around `ax`: assert with `ax expect`, watch with
   `ax state --watch`, read `--json`.
4. Learn before editing: read the brief, then the code around.
5. Check, run, test: `cargo check`, `ax dev <app>`, tests that prove it.
6. Clean up only what you made: a throwaway app runs with
   `AIORT_DATA=<dir>` for every verb; done, `ax stop` and delete that dir.

## The loop

| Step | Verbs |
|---|---|
| understand | `ax agent`, `cargo doc -p aio-rs --no-deps` (read the files, never `--open`) |
| run | `ax dev <app>`, `ax start <app>` |
| look | `ax state`, `ax events`, `ax surface`, `ax shot`, `ax logs` |
| act | `ax call`, `ax trigger` |
| prove | `ax expect`, `ax test <app>` |
| debug | [debugging](debugging/README.md) |

Porting an aio app: [migration](migration/README.md), keeping aio's
optimizations ([performance](performance.md)).

## A throwaway run

```sh
AIORT_DATA=/tmp/try-notes ax start notes --fresh
AIORT_DATA=/tmp/try-notes ax call notes counter add 2
AIORT_DATA=/tmp/try-notes ax expect notes counter count eq 2
AIORT_DATA=/tmp/try-notes ax stop notes
rm -rf /tmp/try-notes
```

The same `AIORT_DATA` on every verb reaches the same copy; its data, logs
and cache stay in that folder (only its sockets live under
`$XDG_RUNTIME_DIR/aio-rt`, else the temp dir).
