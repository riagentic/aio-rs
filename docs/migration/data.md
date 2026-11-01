# aio app data: the format aio-rs reads and writes

A port opens the aio app's data as is, and the aio app can still open it
after its port ran. This is aio's on-disk format as aio's code has it
(paths are under aio's `src/`), and the rules aio-rs follows. Every native
build reads and writes this format (`aio::compat::data`): it needs no
feature. Only aio's one-time move of a scattered layout (7) and
`#[on_aio]` need aio-rs's feature `compat` (`aio::compat`, with aio's
wire: [wire](wire.md)). Golden files: aio's `tests/fixtures/v1.0.9-*`,
`v1.0.11-*`.

## Where the data lives

- Home: `--home` > `--profile`/`AIO_PROFILE` (`<base>-<profile>`) > `$AIO_APPS_DIR/<appId>` > `~/.<appId>` (`server/app-dirs.ts`). `homedir()` is `$HOME`, else `$USERPROFILE`, on every OS. `--instance=<n>`: `~/.aio-instances/<n>`.
- `<home>/data/` (0700, the backup unit): `state.db` (+ `-wal`, `-shm`), `auth.db` (`users`, `one_shot_tokens`, `sessions`), `journal` (+ `.base`, `.replayed`), `tls/`, `files/blobs`, `app.key`, `control.key`, `meta.json`, `reports/`, `update-pending.json`.
- `<home>/{cache,logs,app,backups}`, `launch.json`; the claim `<home>/.aio-instance.lock` (exclusive OS lock) + `.aio-instance.json`.
- `meta.json` = `{appId, profile?, aio, app?, createdAt, updatedAt, dbPath?}` (tmp + rename, 0600). Another appId or profile: aio refuses to start.
- Android standalone keeps no `state.db`: `filesDir/aio-store` (native store): one file per key, `<key with non-[A-Za-z0-9._-] as _, 64 at most>.<Java hashCode, hex>.json`, the app's under `aio:<appId>` (the document `{"<cell>": {...}, "__versions": {...}}`). A port installed over the APK makes it its `state.db` once, at its first start (`aio::compat::data::import_android`); aio's files are kept.
- The Electron profile is outside the home: `~/.config|~/Library/Application Support|%APPDATA%/<profile>`.

## state.db

- PRAGMAs on open: WAL, `synchronous=NORMAL`, `busy_timeout=5000`, `foreign_keys=ON`. `user_version` and `application_id` are never used by aio.
- `aio_kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)`; values are plain `JSON.stringify` (Date → ISO string, Map/Set → `{}`, `undefined` dropped, NaN/±Infinity → `null`; BigInt refused). No wrapper.
- Snapshot (`persistKey`, default `state`): `single` = one row `state` holding `{"<cell>": {...}}`; `multi` = one row per cell, key `state\x1f<cell>`. aio reads both and adopts the other layout.
- In the document: `__*` keys are carried along, never restored; unknown cells are kept; a slice may hold `__aio_status`.
- Meta rows `<appId>:__<name>`: `__schema` = 1, `__persistKey`, `__versions` `{cell: int}` (monotonic), `__shapes` (FNV-1a), `__layout`, `__journal_wm` (+ per cell), `__journal_gen`.
- Other tables: `aio_schema` (version 1), `aio_store_gen` + triggers on `aio_kv` (journal on), `aio_synced_tables`, `sync_ops`, `sync_snapshots`, `sync_meta`, `sync_compacted_ids`, the app's `db:` tables.
- Not persisted by aio: schedules, acks. The offline queue lives in the browser's `localStorage`.

## What aio-rs does

1. Resolves a ported app's home exactly as aio does, its own `[home]` rules included, and opens its store, `<home>/data/state.db` or the app's `[data] store` (aio's `dbPath`).
2. Takes `<home>/.aio-instance.lock` (exclusive) and writes `.aio-instance.json`, so aio and aio-rs never run on the same data at once; keeps `meta.json`'s `appId` and `profile`.
3. Starts each cell from aio's snapshot (both layouts) at its `__versions` stamp, migrating like any older data. All of an app's cells share the one file, each reading its own slice, or several (`#[aio_cell]` fields, below).
4. On every commit, in the same transaction as its own log: rewrites the cell in aio's layout with `JSON.stringify` semantics (its serde names must be aio's field names), without its `#[transient]` fields (aio's `persist: { exclude }`), keeps unknown cells and `__*` keys, stamps `__schema` = 1, `__persistKey` and a monotonic `__versions`. Cells committing at once never lose each other's writes to the one `state` row.
5. Keeps its history in its own tables beside them (aio ignores unknown tables); never writes `aio_kv` keys starting with `state\x1f` other than cells, never changes `aio_schema` or aio's `sync_*`/`db:` tables except in aio's shapes.
6. An aio journal not yet folded into the snapshot is replayed by aio only: aio-rs opens such a file once aio has folded it, or says to start the aio app once.
7. Moves a scattered pre-home layout in once, as aio does (`app-dirs-migrate.ts`, `aio::compat::scatter`, feature `compat`; without it the boot says when `./data.db` is there and leaves it): `./data.db` (+ sidecars, `.journal`, `.journal.wm`) to the store, `$XDG_DATA_HOME|~/.local/share/<appId>/auth.db` to `data/auth.db`, `./.aio-tls/*` to `data/tls/`, `./.aio/log/*` to the logs. Never onto a target that exists; a database and its WAL as one set under a `<to>.moving` record; a copy across filesystems takes the name only whole, with the source's time; a WAL stranded by a death is carried over only under its record and when the store wasn't written since. Better than aio: the start folder's `.aio/` and `.aio-tls/` come along only when that folder holds the app's `data.db` or journal (any aio project has a `.aio/`), and a journal whose watermark can't move stays with it.
8. The app's own tables in the store file (beside `aio_kv`, made by aio's `app.db`) stay the app's: read and written through `aio::db::Db` on the same file, in aio's shapes.

## The app's side

```toml
[app]
id = "notes"
port_of = "notes"          # the aio app's appId: its home and store

[data]
store = "notes.db"         # aio's dbPath, a file in <home>/data (default state.db)
retire = ["state.db"]      # set aside at boot as state.db.pre-migration (+ -wal, -shm), never deleted

[home]                     # the home rules the aio app had in its own code
env = "NOTES_HOME"         # a variable naming the home
portable = "notes-data"    # a folder beside the binary that is the home (not in a debug build)
dev = ".notes-dev"         # a debug build's home, without a profile
```

- Home: `--home` > `--profile`/`AIO_PROFILE` (counted from the `env`/`portable` home, else aio's base, never from `dev`) > `env` (trimmed, `~` expanded) > `portable` > `dev` > `$AIO_APPS_DIR/<appId>` > `~/.<appId>`; `--instance` as in aio. `aio::paths::plan_app(&cfg, &ask)` decides it, `AppDirs::planned` claims it, `AppDirs::store` is the store's path. The boot report says who decided (`[home] env`, `[home] portable`, `[home] dev`).
- Cells: `Source::<T>::for_app(&cfg, &dirs)` opens the app's first cell (retiring `[data] retire` first); every other cell opens on the same file: `Handle::<C>::open_aio(&dirs.store, dirs.port_of.as_deref().unwrap())`. A cell's `ID` is its aio cell name, its serde names aio's field names, its `version` aio's `__versions` stamp.
- One cell for many of aio's (an aio app's state split over slices): `#[aio_cell] settings: Settings` on a field whose type is a `#[cell(persist, id = "settings", version = 8, migrate = …)]` struct (no `#[methods]` needed) makes that field aio cell `settings`'s whole slice. Each slice is read at its own `__versions` stamp and migrated by its type's `migrate` (a newer stamp refuses the boot, the file kept; an older one backs the file up first as `<store>.v1-<cell>-v<stamp>.bak`), its type's `#[transient]` fields are never written (aio's `persist: { exclude }`), its `#[table]` fields are its `db:` tables. Every commit writes each slice it changed in aio's shape and layout, stamped at its type's version, in the one transaction (a slice that can't be written refuses the call, none written); `__*` keys and unknown slices are kept. The cell's other fields are aio-rs's only (its log and snapshots, never `aio_kv`), kept when aio's slices change under it. The cell's own `id`/`version`/`migrate` are its aio-rs history's; it reads no slice of its own id, and `#[table]` goes on the slice types.
- Bound `db:` tables (aio's `db: { contacts: table(…) }` on a cell's array field, or `"cell.field": { table, shape: "map" }`): `#[table("contacts")]` on that `Vec` field, `#[table("cell_field", key = "pk")]` on a map field. The field is the table's rows (`SELECT *`, numbers, strings, nulls; an empty table not in `aio_synced_tables` leaves the slice's array, as aio adopts it); each commit writes what changed by key (all rows without one) in the same transaction, marks the table in `aio_synced_tables` and leaves the field out of `aio_kv`, as aio does. A file without the table keeps the field in the slice, where aio picks it up. A row aio couldn't store (a column the table lacks, a bool, an object) refuses the call. aio-rs's own history keeps the field as any other. The same binding works on an aio-rs store (`Store::open`, no aio file): the table is declared once the store is set up, each commit writes its rows in the same transaction, rows the app's own SQL changed come in at boot as a new event, and a boot that migrated the cell writes the tables again from the migrated state instead of reading their older rows. Binding a field to one of aio-rs's own tables refuses the boot.
- Declared tables on a new file (`columns = "…"`): the file gets aio's `aio_kv` first, then the tables, so the boot knows it as aio's (as aio makes it); a file the boot would refuse (another program's tables, a newer schema) gets none.
- What aio changed (the first open, or aio ran since) comes in as one `$rewind` event, checked as a call's result is: the cell's `#[on_aio]` method (`pub fn (&mut self)` in `#[methods]`, a `cx: &Cx` may follow; feature `compat`, else the build fails saying so) runs on it first and settles what aio's own boot would (ids past every row, staged changes, its leftovers), then the `#[invariant]`s check it. A refused hook or invariant refuses the boot, `aio_kv` as aio left it. Without a change from aio the hook doesn't run; replay takes the settled state from the event.
- Retired cells: `#[cell(persist, retire = ["drafts", "draftsSeen"])]` on the cell that replaces them deletes, at each of its boots, their slices (either layout), their `__versions` stamps and their aio-rs history (`log`, `snap`, `floor`, `sched`, `ops`), in one transaction with `secure_delete`, the WAL emptied after: a sealed secret a stale copy held is gone from the file, not only unlinked. Retiring a cell its own `#[aio_cell]` fields read refuses the boot. Not touched: the app's tables (plain SQL) and `.bak` copies made before (the user's backups). aio reads the file as before: a cell it doesn't know was never its, and its own stay.
- App tables: `aio::db::Db::open(&dirs.store, Opts::default())`, then plain SQL (`CREATE TABLE IF NOT EXISTS`, as aio made them). aio-rs's own tables (`log`, `snap`, `floor`, `sched`, `ops`) must not be among them: such a file is refused and kept.
- Not ported: aio's `profiles: false` (refusing `--profile` in a shipped build) and `__shapes` stamps for aio's dev-mode shape check.

## Durability: what each saves when

| | aio | aio-rs |
|---|---|---|
| a call returns | the state is in memory; the snapshot row is written after `persistDebounceMs` (100 ms) | the call's event and the cell's `aio_kv` row are committed before it returns |
| a held write | every write is debounced | only `#[coalesce]` methods are held (up to `aio::PERSIST_DEBOUNCE`), flushed by a call that can't wait, a time-travel step and the close |
| a kill in between | lost unless `journal: true` (replayed at the next boot) | nothing committed is lost; a held `#[coalesce]` run may be |
| power loss | WAL + `synchronous=NORMAL`: the newest commits may be lost | the same with `[data] durability = "normal"`; `"full"` syncs each commit (phones always) |
| a damaged file | `checkIntegrityOnBoot` asks `quick_check` | every boot asks `quick_check` and recovers what it can |

## What makes aio refuse a file (aio-rs never does these)

`<appId>:__schema` > 1 · `aio_schema.version` > 1 (apps with `db:` tables or sync cells) · a snapshot that isn't a JSON object · `meta.json` with another appId/profile · the home lock held · (dev only) a changed shape without a matching `__shapes` stamp · a declared `db:` table changed so a NOT NULL column can't be filled.
