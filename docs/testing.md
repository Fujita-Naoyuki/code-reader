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

テスト数：385 件（うちユーザイベント起因テスト 245 件）。egui を使わない処理のユニットテストは、`src/core/` の各ファイルにあります。

## テスト時に差し替えている処理（モック）

| 処理 | 本番 | テスト |
|---|---|---|
| クリップボード、フォルダ選択ダイアログ、ブラウザ起動、状態ファイルの場所、カレントディレクトリ、LSP クライアント生成 | `platform::SystemPlatform`（`src/platform.rs`） | `FakePlatform`。`platform::install` でテストスレッドにだけ差し込む |
| LSP サーバー | `rust-analyzer` / `gopls` プロセス | 次の3方式から選ぶ。① チャネルレベルの偽物（`LspClient::in_memory`）② 実ワーカー＋スクリプト化した JSON-RPC サーバー（`lsp::register_test_process` で起動コマンド名に結び付け、OS パイプで接続）③ 存在しない実行ファイル・LSP ではない実行ファイルの実起動 |
| git | 実行 | 実行（一時リポジトリ。`GIT_CONFIG_GLOBAL` / `GIT_CONFIG_NOSYSTEM` で開発者の設定を無視） |
| 時間 | 実時間 | 外部変更スキャン（1秒ごと）は既定で先送りし、必要なテストだけ `next_workspace_change_poll` を前倒しして起動する。LSP のタイムアウトは要求開始時刻を巻き戻して再現する |

テストのための本番コードへの変更は、上記の差し替え点（`platform.rs`、`LspProcess` による LSP プロセス I/O の抽象化）だけです。挙動は変えていません。

egui_kittest は AccessKit 経由で UI を読むため、テストは常に AccessKit が有効な状態で動きます。そのため、実在しないウィジェットにフォーカスを置くコードはテストでパニックします（[乖離 D07](discrepancies.md)）。以前はテストビルド限定の回避コード（`register_focused_test_node`）を入れていましたが、D07 の修正に伴い削除しました。

## 書き方の約束

- 入力は 1 イベントにつき 1 フレームで送る（`TestApp::click` / `key` / `type_text` など）。同じフレームに複数のイベントが必要なときだけ `keys_in_one_frame` を使う。
- ステータスメッセージはバックグラウンド処理ですぐ上書きされるため、`assert_status` は「これまでに表示されたメッセージの履歴」を検査する。
- 状態を直接書き換えて分岐に入るのは避ける。例外は次の3つで、いずれもコメントで理由を明記している。
  - 時間を進める操作（スキャン時刻の前倒し、LSP 要求開始時刻の巻き戻し）
  - 観測用の目印として `status_message` に書き込む操作
  - 到達不能な状態を説明するためのもの（D11）
- 意図と異なる現状の挙動を固定するテストには、`DISCREPANCY(Dxx)` のコメントを付ける（[discrepancies.md](discrepancies.md)）。D01〜D16 はすべて修正済みで、テストは意図どおりの挙動を検証している。

## カバレッジ

`cargo llvm-cov` による行カバレッジ（テストコード自身は除く）。

| 対象 | 開始時 | テスト追加後 | リファクタリング後 |
|---|---|---|---|
| 全体 | 25.2% | 95.4% | 97.56% |
| `core/lsp/` | 9.3% | 95.1% | 95.72% |
| `core/git.rs` | — | — | 96.98% |
| `core/csv.rs` | — | — | 98.76% |
| `core/diff.rs` | — | — | 97.52% |
| `core/text.rs` | — | — | 98.62% |

ファイルごとの最新の値は `cargo llvm-cov --summary-only` で確認できます。

### 残っている未カバー行の分類

- **防御的な分岐**：呼び出し元の条件で起こらない `else` / `return None`。例：1 ルートしかないときの削除拒否（UI に × が出ない）、ウィンドウを開いたままリポジトリが消えたときのガード。
- **GFM で生成されないノード**：数式、ブロックレベルの画像、断片パースの失敗など。
- **外部要因**：バックグラウンドスレッドの異常終了、`git` が出力を返さない場合、リリースビルドの分岐。テスト環境では再現できないか、非決定的。
- **実際の OS を呼ぶ部分**：`platform.rs` の `SystemPlatform`、`app.rs` と `main.rs` の起動処理、`rust-analyzer` の探索。テストでは差し替えている。
- **到達できるが未テスト**：履歴の参照バッジの省略、差分選択テキストの細部、スクロールバーのレイアウトの一部、未保存確認での保存失敗など。必要に応じて追加する。

未使用コードは削除済みです。`allow(dead_code)` はなく、未使用の項目はコンパイラの警告になります（`scripts/check.ps1` では失敗します）。
