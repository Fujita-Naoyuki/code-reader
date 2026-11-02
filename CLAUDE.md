# Code Reader — notes for AI agents

A local egui (eframe 0.34) app for reading code: workspace tree, search, Rust / Go
definitions through their language servers, Markdown preview, CSV grid, Git changes.
User-facing docs are in `README.md` (Japanese); engineering docs are in `docs/`.

## Verify every change

```powershell
pwsh scripts/check.ps1            # cargo fmt --check, clippy -D warnings, cargo test
pwsh scripts/check.ps1 -Coverage  # also cargo llvm-cov --summary-only
```

All three must pass before a commit. Tests take about a minute; the Git tests run the
real `git` binary against temporary repositories.

## Where things are

Logic without egui is in `src/core/` and can be unit tested without a window:

| File | What it holds |
|---|---|
| `core/text.rs` | `EditorTextBuffer`, line index, position math, bracket matching |
| `core/csv.rs` | CSV parsing, serialization, row / column edits |
| `core/git.rs` | Running `git`, status / history parsing, diff text, push-or-publish |
| `core/diff.rs` | Unified diff text into display rows, selection text |
| `core/markdown/` | Document model, source-mode spans, definitions, links, Mermaid |
| `core/lsp.rs` | Language server client (worker thread, JSON-RPC) |
| `core/navigation.rs` | Back / Forward history |
| `core/workspace_files.rs` | File kinds by path, change snapshots, new-file names |
| `core/session.rs` | Workspace sessions and the state file |
| `core/process.rs`, `core/file_tree.rs`, `core/symbol*.rs` | Child processes, file discovery, symbols |

`src/platform.rs` holds OS side effects (clipboard, folder picker, URLs, state path, LSP
process); tests swap it for `FakePlatform`. `src/languages/` has tree-sitter symbols and
highlighting; definitions always come from the language servers.

The UI is `src/ui/app_view.rs` (`CodeReaderApp`, the frame layout) plus child modules in
`src/ui/app_view/`. They are flat and read the parent's items with `use super::*`; what
other modules need is `pub(super)`.

| To change | Look in |
|---|---|
| A pane's content, saving, unsaved edits | `editor_pane.rs` (`EditorContent`), `panes.rs` |
| Shortcuts in Help / the status bar, key checks | `keymap.rs` (handlers live with their feature) |
| Which dialog is in front, who gets Escape | `modal.rs` |
| Status bar messages | `status.rs` (`status_message` for action results, `set_background_status` for notices) |
| Regular editor, find bar | `source_editor.rs`, shared parts in `editor_style.rs` |
| Large-file editor | `virtual_editor.rs` (frame), `virtual_input.rs` (keys), `virtual_pointer.rs` (mouse), `virtual_paint.rs` |
| Markdown | `markdown_view.rs` (modes, panes), `markdown_render.rs`, `markdown_source.rs`, `markdown_search.rs` |
| CSV grid | `csv_grid.rs` |
| Diffs, Git history | `diff_view.rs`, `diff_selection.rs`, `git_history_view.rs` |
| Git sidebar and commands | `git_panel.rs`, `git_actions.rs`, `git_windows.rs` |
| Definitions, references | `lsp.rs` (`LspSession`, `LspLanguage`) |
| Search Everywhere, references list, workspace search | `search.rs` |
| File tree, sessions, bookmarks | `workspace.rs`, `sessions.rs`, `navigation.rs` |
| Report button, performance records | `diagnostics.rs`, `editor_debug.rs` |
| Tests driving the app with user events | `event_tests/` (`support.rs` has `TestApp`) |

## Tests

- Write behavior tests as user events through `TestApp` (`support.rs`): click, key,
  type, drag. One event per frame; `assert_status` checks the status history.
- Do not poke app state to reach a branch. The exceptions (moving timers forward,
  reaching states no input can reach) carry a comment saying why.
- A test that pins behavior known to be wrong gets a `DISCREPANCY(Dxx)` comment and an
  entry in `docs/discrepancies.md`.
- See `docs/testing.md` for the fakes (Platform, LSP modes, git isolation) and coverage.

## egui pitfalls this code base already hit

- `InputState::consume_key(mods, key)` ignores extra Shift / Alt. Check the modified
  combination first (Shift+F6 before F6, Ctrl+Shift+Z before Ctrl+Z).
- egui clears focus at the start of the frame that carries a plain Escape; ask
  `memory.had_focus_last_frame(id)`.
- Only request focus for an id whose widget exists in the same frame. AccessKit (always
  on under egui_kittest) panics on a focused id without a node.
- egui may run a pass twice with the same input; toggles in one frame must not double.

## Conventions

- Keep moves and behavior changes in separate commits; one purpose per commit.
- Do not push; a human pushes.
- Comments explain why, not what. Match the surrounding style.
