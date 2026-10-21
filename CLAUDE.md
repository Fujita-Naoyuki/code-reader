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

- `src/core/` — logic without egui (sessions, file tree, LSP client, Markdown parsing,
  symbols). Prefer adding testable logic here.
- `src/ui/app_view.rs` — the app (`CodeReaderApp`) and editor panes (`EditorPane`). Being
  split up; see `docs/refactoring-plan.md` for the target layout.
- `src/platform.rs` — OS side effects (clipboard, folder picker, URLs, state path, LSP
  process). Tests swap it for `FakePlatform`.
- `src/ui/app_view/event_tests/` — tests that drive the real app with user events.

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
