# ユーザイベント起因テスト

アプリ本体（`CodeReaderApp`）を `egui_kittest` のハーネス上で実際に動かし、クリック・キー入力・テキスト入力・ファイルシステムの変更といった「ユーザが起こせる出来事」だけで各機能を検証するテスト群です。

```powershell
cargo test                      # すべてのテスト（約1分）
cargo test event_tests          # ユーザイベント起因テストのみ
cargo test event_tests::git     # 機能単位
cargo llvm-cov --summary-only   # カバレッジ（cargo install cargo-llvm-cov が必要）
```

Git のテストは実際の `git` コマンドを一時リポジトリに対して実行するため、`git` が PATH 上に必要です（グローバル・システム設定は無視します）。

## 構成

`src/ui/app_view/event_tests/`（`app_view` の子モジュールなので非公開フィールドも参照できる）

| ファイル | 内容 |
|---|---|
| `support.rs` | `TestApp`（一時ワークスペース＋ハーネス）、`FakePlatform`、入力ヘルパー |
| `scripted_lsp.rs` | 本物の LSP フレーミング（Content-Length＋JSON-RPC）を話す偽サーバー |
| `workspace.rs` | ファイルツリー、新規作成・リネーム・削除、ルートフォルダ、ドラッグ＆ドロップ、外部変更検知 |
| `sessions.rs` | New Workspace、Recent Workspaces、ワークスペース名変更 |
| `search.rs` | Search Everywhere、ワークスペース全文検索、ブックマーク |
| `editor.rs` | 通常エディタ（入力補助、保存、ファイル内検索、ブックマーク） |
| `virtual_editor.rs` | 大容量ファイル用の仮想エディタ |
| `markdown.rs` / `markdown_rendering.rs` | Markdown プレビュー（モード、ページ、リンク、脚注、表、画像、検索） |
| `csv.rs` | CSV グリッド |
| `git.rs` / `diff.rs` | Git サイドバー、差分ビュー、履歴 |
| `lsp.rs` | F12・参照・Back/Forward（チャネルレベルの偽 LSP） |
| `lsp_protocol.rs` | 実際の LSP ワーカー＋JSON-RPC 偽サーバー |
| `windows.rs` | Help、AI Cmd、Report |

テスト数：ユニットテスト 141 件＋ユーザイベント起因テスト 242 件＝383 件（D01〜D16 の修正後）。

## テスト時に差し替えている処理（モック）

| 処理 | 本番 | テスト |
|---|---|---|
| クリップボード、フォルダ選択ダイアログ、ブラウザ起動、状態ファイルの場所、カレントディレクトリ、LSP クライアント生成 | `platform::SystemPlatform`（`src/platform.rs`） | `FakePlatform`。`platform::install` でテストスレッドにだけ差し込む |
| LSP サーバー | `rust-analyzer` / `gopls` プロセス | 次の3方式から選ぶ。① チャネルレベルの偽物（`LspClient::in_memory`）② 実ワーカー＋スクリプト化した JSON-RPC サーバー（`lsp::register_test_process` で起動コマンド名に結び付け、OS パイプで接続）③ 存在しない実行ファイル・LSP ではない実行ファイルの実起動 |
| git | 実行 | 実行（一時リポジトリ。`GIT_CONFIG_GLOBAL` / `GIT_CONFIG_NOSYSTEM` で開発者の設定を無視） |
| 時間 | 実時間 | 外部変更スキャン（1秒ごと）は既定で先送りし、必要なテストだけ `scan_workspace()` などで明示的に起動する。LSP のタイムアウトは要求開始時刻を巻き戻して再現する |

本番コードへの変更は、上記の差し替え点（`platform.rs`、`LspProcess` による LSP プロセス I/O の抽象化）と、テストビルド限定の `register_focused_test_node`（[乖離 D07](discrepancies.md#d07-実在しないウィジェットへのフォーカス高要実機確認) の回避）だけです。挙動は変えていません。

## 書き方の約束

- 入力は 1 イベントにつき 1 フレームで送る（`TestApp::click` / `key` / `type_text` など）。同じフレームに複数のイベントが必要なときだけ `keys_in_one_frame` を使う。
- ステータスメッセージはバックグラウンド処理ですぐ上書きされるため、`assert_status` は「これまでに表示されたメッセージの履歴」を検査する。
- 状態を直接書き換えて分岐に入るのは避ける。例外は次の3つで、いずれもコメントで理由を明記している。
  - 時間を進める操作（スキャン時刻の前倒し、LSP 要求開始時刻の巻き戻し）
  - 観測用の目印として `status_message` に書き込む操作
  - 到達不能な状態を説明するためのもの（D11）
- 意図と異なる現状の挙動を固定するテストには、`DISCREPANCY(Dxx)` のコメントを付ける（[discrepancies.md](discrepancies.md)）。D07 以外は修正済みで、テストは意図どおりの挙動を検証している。

## カバレッジ

`cargo llvm-cov` による行カバレッジ（テストコード自身は除く）。

| 対象 | 開始時 | テスト追加後 | D01〜D16 修正後 |
|---|---|---|---|
| 全体 | 25.2% | 95.4% | 95.4% |
| `ui/app_view.rs` | 20.5% | 96.4% | 96.3% |
| `core/lsp.rs` | 9.3% | 95.1% | 95.3% |

### 残っている未カバー行の内訳

#### `ui/app_view.rs`（未カバー 674 行。下表は関数単位で分類できた 623 行の内訳）

この内訳は D01〜D16 を修正する前の計測です。修正後の未カバーは 709 行です。修正で追加したガード（未保存確認の保存失敗時など）が増えた一方、「乖離による到達不能」のうち D02・D08・D10 の分は到達可能になりました。

| 区分 | 行数 | 主な箇所 | 扱い |
|---|---|---|---|
| 未使用コード | 272 | LSP 非対応言語向けの定義解決（`go_to_definition` の後半、`handle_locations`）、一時ブックマーク（`add_bookmark_from_current_position`）、旧差分描画（`diff_file_header_ui`、`diff_row_ui`、`side_by_side_hunk_ui`、`diff_layout_job`、`diff_line_kind`、`diff_text_format`）、生成されない列挙子（`DiffDisplayRow::Hunk`、`CsvHeaderAction::Select`、`GitDiffScope::Commit` の個別差分）、未使用の `TextBuffer` メソッドなど | 本番から呼ばれない。削除候補 |
| 防御的分岐 | 167 | 呼び出し元の条件で起こらない `else` / `return None`（例：1ルートしかないときの削除拒否は、UI に × が出ないため到達しない）、LSP 応答の `WarmUp` / `Sync` 分岐、ウィンドウを開いたままリポジトリが消えたときのガードなど | 到達不能。整理対象 |
| GFM で生成されないノード | 95 | 断片パースの失敗（GFM では発生しない）、数式、ブロックレベルの画像、断片内の見出しノード、ページ ID のない見出し | GFM 以外の構文を使わない限り到達しない |
| 外部要因 | 30 | バックグラウンドスレッドの異常終了、`git` が出力を返さない場合、リリースビルドの分岐、状態ファイルの親フォルダがない場合 | テスト環境では再現不能・非決定的 |
| 乖離による到達不能 | 23 | D02（Esc 判定）、D08（Tab）、D10（Redo 判定）、R03（Markdown 見出し色）、R04（パンくず省略表示）、R05（スクロールバー当たり判定） | 乖離を修正すると到達可能になる。修正時にテストを追加する |
| 到達可能だが未テスト | 36 | 履歴の参照バッジ省略（4件×最大幅が表示幅にちょうど収まるため実際には発生しない）、差分選択テキストの細部、仮想エディタのスクロールバー2段目のレイアウト判定など | 必要に応じて追加 |

#### その他のファイル

| ファイル | 未カバーの理由 |
|---|---|
| `core/external_definitions.rs`、`core/external_libraries.rs`（ユニットテストで一部カバー）、`features/definition.rs`、`core/workspace.rs` | LSP 非対応言語の定義解決からしか呼ばれない。登録言語は Rust / Go のみで、どちらも LSP 経由なので、UI からは到達不能 |
| `platform.rs`（`SystemPlatform`） | 実際の OS（クリップボード、ダイアログ、ブラウザ、`rust-analyzer` 起動）を呼ぶ部分。テストでは差し替えている |
| `app.rs`、`main.rs` | ネイティブウィンドウの起動処理 |
| `core/lsp.rs` の残り | UNC パス・パーセントエンコード変換の一部、`RUST_ANALYZER` 環境変数や VS Code 拡張内の `rust-analyzer` 探索（環境依存）、送信先ドキュメントが欠けた場合などの防御的分岐 |
| `languages/*.rs`、`core/symbol_index.rs` の残り | `resolve_definition`・`name`・`find_by_name` などの未使用メソッド、tree-sitter が `None` を返す場合（キャンセル時のみ）の分岐 |

未使用コードの一覧は `src/main.rs` の `#![allow(dead_code)]` を一時的に外してビルドすると、コンパイラの警告として確認できます。
