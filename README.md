# Code Reader

Code Reader は、コードを読む作業に寄せた軽量なローカルアプリケーションです。

ワークスペース内のファイルをすばやく開き、検索し、Rust / Go の定義や参照へ移動しながら読み進めることを主な目的にしています。VS Code の代替を目指すというより、コードリーディング、Markdown の確認、CSV の軽い編集、Git 差分の確認を 1 つの画面で行うためのツールです。

## Features

- Workspace
  - 複数ワークスペースの管理
  - 複数 root folder の追加
  - Recent Workspaces からの切り替え
  - ファイルツリーからのファイル作成、リネーム、削除
- Code navigation
  - Go to Definition
  - References 表示
  - Definition / References 移動後の履歴移動
  - ファイルツリー上で現在ファイルを選択表示
- Search
  - Search Everywhere
  - ワークスペース全文検索
  - 現在ファイル内検索
- Editor
  - 大きなファイル向けの仮想化表示
  - 文字選択、Copy / Paste、Undo / Redo
  - Bookmarks
  - 行番号、インデントガイド、シンタックスハイライト
- Markdown
  - Preview / Markdown / Both 表示
  - Pages / Outline
  - ページ移動
  - Mermaid 表示
- CSV
  - Grid 表示
  - セル、行、列の選択
  - Copy / Paste
  - Undo / Redo
- Git
  - Repository 一覧
  - Changed files
  - Stage / Unstage
  - Diff
  - History
  - Commit details
  - Push / Publish Branch
  - Branch 切り替え、Branch 作成
- AI Cmd
  - 現在のワークスペース構成から AI CLI 用コマンドを生成

## Supported Files

ワークスペース探索では、対象ファイルを意図的に絞っています。これは大きなリポジトリを開いたときの初期表示や検索を軽くするためです。

主な対象:

- Rust: `.rs`
- Go: `.go`
- Markdown: `.md`, `.mdx`
- CSV / TSV: `.csv`, `.tsv`
- Text / config: `.txt`, `.json`, `.jsonc`, `.toml`, `.yaml`, `.yml`, `.lock`
- Common project files:
  - `README`, `README.md`
  - `TODO`, `TODO.md`
  - `Cargo.toml`, `Cargo.lock`
  - `go.mod`, `go.sum`
  - `.env`, `.env.*`
  - `.gitignore`, `.gitattributes`
  - `Dockerfile`, `Makefile`, `LICENSE`

現時点では、HTML / CSS / JavaScript / SQL / XML などは主な対象外です。

## Supported Languages

言語機能は Rust / Go を中心に対応しています。

| Language | Features | Required tool |
| --- | --- | --- |
| Rust | definition / references | `rust-analyzer` |
| Go | definition / references | `gopls` |

定義ジャンプや参照検索は LSP に寄せています。`rust-analyzer` や `gopls` が起動直後に解析中の場合、初回の F12 が少し遅くなることがあります。ステータスバーに LSP の状態が表示されます。

## External Tools

一部機能では、外部ツールが利用できることを前提にしています。

- Git features: `git`
- AI Cmd: `claude` など任意の CLI 向けの起動コマンドを生成してコピー

これらの外部ツールは Code Reader には同梱していません。

## ライセンス

Code Reader は MIT License で提供されています。詳細は `LICENSE` を参照してください。

依存ライブラリや同梱フォントのライセンス表記は
`THIRD_PARTY_NOTICES.md` に記載しています。

## Acknowledgements

Code Reader の Markdown preview は
[Markdown Experience Guidelines (MDXG)](https://github.com/vercel-labs/mdxg)
のアイデアを参考にしています。MDXG は Apache License 2.0 で公開されています。

## Build

```powershell
cargo build --release
```

生成される実行ファイル:

```powershell
.\target\release\code-reader.exe
```

## Release package

Windows 向けの配布 zip を作る例です。

```powershell
cargo build --release

Remove-Item dist\code-reader -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force dist\code-reader

Copy-Item target\release\code-reader.exe dist\code-reader\
Copy-Item README.md,LICENSE,THIRD_PARTY_NOTICES.md dist\code-reader\
Copy-Item assets\fonts\OFL-1.1.txt dist\code-reader\

Compress-Archive -Path dist\code-reader -DestinationPath dist\code-reader-windows.zip -Force
```

## Development

```powershell
cargo fmt
cargo check
cargo test
```

## Shortcuts

主なショートカットです。アプリ内では `Help` または `F1` から確認できます。

| Shortcut | Action |
| --- | --- |
| `F12` | Go to Definition |
| `Alt+Left` | Back |
| `Alt+Right` | Forward |
| `Ctrl+F` | Find current file |
| `Ctrl+Shift+F` | Workspace Search |
| `Ctrl+P` | Search Everywhere |
| `Ctrl+Shift+O` | Search Everywhere |
| `Shift x2` | Search Everywhere |
| `Ctrl+S` | Save |
| `Ctrl+A` | Select all |
| `Ctrl+C` | Copy selection |
| `Ctrl+V` | Paste |
| `Ctrl+Z` | Undo |
| `Ctrl+Y` | Redo |
| `Ctrl+Shift+Z` | Redo |
| `F1` | Help |
| `F2` | Rename selected file |
| `Delete` | Delete selected file |
| `PageUp / PageDown` | Page scroll |

Markdown preview:

| Shortcut | Action |
| --- | --- |
| `Ctrl+PageUp` | Previous page |
| `Ctrl+PageDown` | Next page |

CSV Grid:

| Shortcut | Action |
| --- | --- |
| `Ctrl+C` | Copy selected cells / rows / columns |
| `Ctrl+V` | Paste |
| `Delete` | Delete selected cells / rows / columns |
| `Ctrl+Z` | Undo CSV grid edit |
| `Ctrl+Y` | Redo CSV grid edit |
| `Ctrl+Shift+Z` | Redo CSV grid edit |

## Design Notes

- 大きなファイルでは、全行を通常の text editor として描画せず、可視行中心の仮想化ビューを使います。
- Rust / Go の定義解決と参照検索は LSP に寄せています。
- ワークスペース切り替え時の負荷を抑えるため、外部依存の解析は必要以上に起動時へ寄せない方針です。
- Search Everywhere はワークスペース内のファイル、シンボル、ブックマーク、履歴を素早く開くための入口です。
- UI は、コードを読む作業でよく使う操作を画面上に置きつつ、詳細情報は Report や Help に寄せています。

## Limitations

- 本格的な言語機能は Rust / Go が中心です。
- Rust / Go の F12 / References は `rust-analyzer` / `gopls` の状態に依存します。
- 起動直後やワークスペース切り替え直後は、LSP の準備が終わるまで定義ジャンプに時間がかかることがあります。
- HTML / CSS / JavaScript などの一般的な Web フロントエンドファイルは、現時点では主な対象外です。
- 高度な Git 操作やターミナル統合は限定的です。
