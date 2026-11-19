<!-- piki:skills:start -->
<!-- Managed by piki — edits inside this block are overwritten the
next time a skill here changes. Decline a skill from piki's repo
settings to remove it instead of editing here. -->

### piki-ctl — open files in piki's UI

If `piki-ctl` is available on PATH, you can ask piki to open a file in its
own file preview instead of just printing the file's contents:

```
piki-ctl open file <path>
```

`<path>` is resolved against your current working directory. Use this when
the user in a piki-managed terminal asks you to "show", "open" or "pull up"
a file. If the command isn't found, piki-ctl simply isn't installed in this
environment — fall back to your normal behavior without retrying.
<!-- piki:skills:end -->

## Command palette constraint
Every user-facing action must be a variant of `Command` in `src/tui/actions.rs`.
Palette rows are generated from `Command::iter()` (opened with `Ctrl+G k`) and
dispatch goes through the exhaustive `Command::execute`, so adding a variant
without palette metadata (`id`/`description`/`shortcut`) or without an
implementation fails to compile. The `cmd_*` helpers are module-private so
`run.rs` cannot bypass the palette, except `cmd_select_tab` which serves the
digit shortcuts (1-9 are a closed family, not palette commands). Modal
scroll-mode keys (`/`, `y`, …) are not global commands and stay out too.
