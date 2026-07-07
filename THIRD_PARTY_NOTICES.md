# Third Party Notices

Code Reader includes third-party open source software and bundled font assets.

This file is intended for distribution with Code Reader binaries. Dependency
versions are managed by `Cargo.lock`.

## Rust Dependencies

Code Reader depends on Rust crates distributed under permissive open source
licenses, primarily:

- MIT
- Apache License 2.0
- BSD-2-Clause / BSD-3-Clause
- Zlib
- ISC
- BSL-1.0
- CC0-1.0
- Unicode License v3
- Unlicense

Notable dependency license choices:

- `self_cell` is licensed as `Apache-2.0 OR GPL-2.0-only`; Code Reader uses it
  under the Apache License 2.0 option.
- `epaint_default_fonts` includes font assets under `OFL-1.1` and
  `Ubuntu-font-1.0` in addition to Rust code under `MIT OR Apache-2.0`.

Direct Rust dependencies:

| Package | License |
| --- | --- |
| `arboard` | `MIT OR Apache-2.0` |
| `eframe` | `MIT OR Apache-2.0` |
| `egui_extras` | `MIT OR Apache-2.0` |
| `fuzzy-matcher` | `MIT` |
| `ignore` | `Unlicense OR MIT` |
| `image` | `MIT OR Apache-2.0` |
| `markdown` | `MIT` |
| `mmdflux` | `MIT` |
| `rfd` | `MIT` |
| `serde` | `MIT OR Apache-2.0` |
| `serde_json` | `MIT OR Apache-2.0` |
| `toml` | `MIT OR Apache-2.0` |
| `tree-sitter` | `MIT` |
| `tree-sitter-go` | `MIT` |
| `tree-sitter-rust` | `MIT` |
| `webbrowser` | `MIT OR Apache-2.0` |

## Bundled Fonts

### Moralerspace Neon Regular

- File: `assets/fonts/MoralerspaceNeon-Regular.ttf`
- Project: https://github.com/yuru7/moralerspace
- Copyright: Copyright (c) 2022 Yuko Otawara
- Reserved Font Name: "Moralerspace"
- License: SIL Open Font License, Version 1.1
- License text: `assets/fonts/OFL-1.1.txt`

The font is bundled as a Japanese glyph fallback for Code Reader's editor and
Markdown views.

## External Tools

Some Code Reader features invoke tools installed separately by the user. These
tools are not bundled with Code Reader.

- `git`
- `gopls`
- `rust-analyzer`
- AI CLI tools such as `claude`

## Design References

### Markdown Experience Guidelines (MDXG)

Code Reader's Markdown preview behavior was informed by ideas from Markdown
Experience Guidelines (MDXG).

- Project: https://github.com/vercel-labs/mdxg
- License: Apache License 2.0
