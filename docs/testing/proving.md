# Proving the tests

A green suite says the tests passed, not that they would notice a bug.
aio-rs proves its own: `scripts/mutants` plants known breaks, one at a
time, and requires a named test to go red for each (aio's
`check:mutations`, carried further: some breaks go into a built package,
not the source).

## The ledger

`scripts/mutants.toml` lists the invariants whose silent loss would cost
data, security or a shipped app. Each entry names the line that enforces
it, that line broken, and what must then fail:

| `in` | The break | Must fail |
|---|---|---|
| `source` | `find` replaced by `put` in the repository's `file` | the test named in `test` (`<package> <target> <test>`): it must run and FAIL; a break that doesn't compile proves nothing |
| `deb` | the template app built (`ax create`, `ax build --deb`), its package unpacked, `find` replaced by `put` in `file` (a program's bytes keep their length), packed again | `ax smoke` of that package, saying `expect`; the untouched package must pass first |

Some of what it holds: an `#[invariant]` broken by a call is refused; a
method that isn't `#[client]` is refused to a network caller; a snapshot
missing a cell is refused (it would wipe that cell); a test can't reach the
user's real data; a seed key the state lacks fails; a failure nobody looked
at fails; a `.deb` whose menu entry starts no program, names an icon it
doesn't ship, or whose bundled page script throws fails its smoke test.

```sh
scripts/mutants              # every break; writes scripts/mutants-ledger.csv
scripts/mutants --fast       # the `fast = true` ones (GitHub CI, on every push)
scripts/mutants --only invariants,deb-page
scripts/mutants --check      # each line found once, each test there; nothing run (./ci.sh)
scripts/mutants --deb dist/notes_0.1.0_amd64.deb   # break this package instead of building one
```

Each break is taken out again before the next. A run cut short can leave
one in: the next run (and `./ci.sh`) refuses to start, naming the file to
restore.

## Reading a result

```text
✓ killed   invariants (41 s): a call that breaks an #[invariant] is refused, never committed
    broken in crates/aio/src/store.rs, caught by aio-rs test:core typed_calls_and_refusals
✗ SURVIVED seed-typo (12 s): a seed key the state lacks fails the test instead of pinning nothing
```

A survivor is the finding: nothing guards that line. Write the test that
goes red, then run the break again. `scripts/mutants-ledger.csv` keeps the
last whole run: each break, where it was caught, its result and time, the
commit and the day.

## Adding a break

Add one when a regression would cost data, security or a release, not for
every line: a short ledger gets run. Pick the one line that enforces the
rule, write the smallest `put` that turns it off (`if false && …`, a
dropped `?`), and name the test that must catch it. `scripts/mutants
--only <id>` before committing; `fast = true` only for a break whose test
builds quickly (CI runs those on every push).

## In an app

The ledger is aio-rs's own. For an app's code, `ax test <app> --mutants`
runs `cargo mutants` (a mutation tool for Rust, `cargo install
cargo-mutants`) over its packages: every function broken in turn, the
ones no test noticed listed, and the run fails; `ax lint` names the tests that check nothing and the
values compared with themselves ([Lint](lint.md)).
