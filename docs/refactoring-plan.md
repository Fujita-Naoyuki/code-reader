# リファクタリング計画（AI によるメンテナンスのため）

AI エージェント（Claude Code など）が、このリポジトリを安全に読み、変更し、検証できる構造にするための計画です。挙動は変えずに構造だけを変えます。安全網には、ユーザイベント起因テスト（[testing.md](testing.md)）を使います。

## 1. 目的

AI がコードを変更するときに起きやすい問題は、次の 4 つです。

- 必要な範囲だけを読めない（巨大ファイルを全部読むか、一部だけ読んで見落とす）
- 変更の影響範囲が分からない（状態がどこから書き換えられるかを追えない）
- 同じ規則が複数箇所に重複していて、一部だけ直してしまう
- 正しく直せたかを、速く機械的に確かめられない

この計画は、これらを構造で防ぐことを目的にします。成功の基準は「7. 完了の指標」にまとめます。

## 2. 現状

| 項目 | 現状 |
|---|---|
| `src/ui/app_view.rs` | 23,796 行。本体 29,122 行の 82%。型 111 個、トップレベル関数 343 個、ユニットテスト約 2,100 行がこの 1 ファイルにある |
| `CodeReaderApp`（アプリ全体の状態） | フィールド 69 個。`impl` は約 5,900 行（ワークスペース、セッション、検索、LSP、Git、各ダイアログ、診断レポート） |
| `EditorPane`（ペインの状態） | フィールド 66 個。通常エディタ、大容量エディタ、Markdown、CSV、差分、Git 履歴のすべての状態を 1 つの構造体に持つ。`impl` は約 7,200 行 |
| 長い関数 | 100 行超が 38 個、200 行超が 11 個。最大は `ui()`（約 620 行）と `virtual_source_editor_ui`（約 500 行） |
| 引数の多い関数 | `#[allow(clippy::too_many_arguments)]` が 8 か所。clippy の警告が 57 件残っている |
| 未使用コード | `main.rs` の `#![allow(dead_code)]` で隠れている。LSP 非対応言語向けの定義解決（`features/definition.rs`、`core/external_definitions.rs`、`core/workspace.rs` など）、旧差分描画、一時ブックマークなど、約 270 行以上（[testing.md](testing.md) の「未使用コード」） |
| 重複 | Rust と Go の LSP 処理がほぼ同じ形で 2 系統ある（`request_rust_lsp_definition` / `request_go_lsp_definition` など約 20 関数）。キー割り当ては、処理側・Help・ステータスバーの 3 か所に別々に書かれている |
| 診断用コード | 大容量エディタのクリック診断や性能計測（`VirtualClickDebug`、`AppPerfDebug` など）が、描画処理の中に混ざっている |
| AI 向けの案内 | `CLAUDE.md` などの作業ガイドがない。テスト・カバレッジの実行方法は [testing.md](testing.md) にあるが、どこを変えると何が変わるかの地図がない |

### 現状の構造が原因だった不具合

D01〜D16・R01〜R09 の多くは、次の構造上の問題から生まれていました。リファクタリングで再発を防ぐ対象です。

| 構造上の問題 | 関連した不具合 |
|---|---|
| ペインの中身の種類（ファイル・差分・履歴）が型で区別されず、フラグとフィールドの組み合わせで表されている | D06（未保存判定で差分・履歴を除外する条件が必要だった）、D16、D08 |
| ダイアログやオーバーレイがそれぞれ別の `Option` / `bool` で、どれが手前にあるかの規則がない | D02（Help と検索ボックスが同じ Esc を受け取った）、D15 |
| キー割り当てが、処理と表示（Help・ステータスバー）で別々に定義されている | D10、D11、D15 |
| ステータスメッセージが 1 本の `String` で、ユーザ操作の結果と裏の処理の通知が上書きし合う | D01 |
| フォーカス ID を呼び出し元の `ui.id()` から作っている | D07 |
| UI に依存しない処理（CSV・Git 出力・Markdown の解析）が UI と同じファイルにある | D04、D13、D14、R03（単体で試しにくく、UI 経由のテストでしか見つからなかった） |

## 3. 目標の構成

```text
src/
  main.rs, app.rs, platform.rs
  core/                       UI に依存しない処理（すべて単体テスト可能）
    text/        buffer.rs (EditorTextBuffer, LineIndex), position.rs, brackets.rs, input_assist.rs
    csv.rs       parse / serialize / selection の編集
    git/         command.rs (GitClient), status.rs, diff_parse.rs, history.rs
    markdown/    （既存）+ source_spans.rs, definitions.rs
    navigation.rs NavigationHistory
    session.rs, file_tree.rs, lsp/ ...（既存）
  ui/
    app_view/
      mod.rs           CodeReaderApp の定義と eframe::App::ui（描画順の骨組みだけ）
      state.rs         アプリ状態のサブ構造体
      keymap.rs        キー割り当ての表（処理・Help・ステータスバーの唯一の定義元）
      modal.rs         手前に出る UI（ダイアログ・オーバーレイ）の enum と Esc / Enter の配分
      status.rs        ステータスメッセージ（操作結果と通知を分ける）
      workspace/       ツリー、ファイル操作、セッション
      search/          Search Everywhere、ワークスペース検索、参照一覧
      lsp.rs           言語共通の LSP セッション管理
      git/             サイドバー、ブランチ・コミット・Discard の各ウィンドウ
      editor/
        pane.rs        EditorPane（共通部分）と EditorContent enum
        source.rs      通常エディタ
        virtual_editor/ 大容量エディタ（入力・描画・スクロールバー・キャッシュ）
        markdown/      プレビュー、ソース、Both、Pages / Outline、検索
        csv_grid.rs
        diff/          差分ビュー（統合・左右並べ・選択）
        git_history.rs
      diagnostics.rs   診断レポートと性能計測
      event_tests/     （既存）
```

目安：

- 1 ファイル 800 行以下（上限 1,200 行）。
- 1 関数 80 行以下。
- `allow(clippy::too_many_arguments)` は使わない（引数は文脈構造体にまとめる）。

`ui/app_view/` の子モジュールは親のプライベートなフィールドを参照できます。そのため、`impl CodeReaderApp` を機能ごとのファイルに分ける作業は、可視性を変えずに機械的に行えます。

## 4. 進め方の原則

1. **挙動を変えない。** 各ステップの前後で全テスト（現在 386 件）が通ること。テストの期待値を変えるのは、テストが内部構造（フィールド名など）を直接参照している場合だけにする。
2. **移動と変更を分ける。** コードの移動だけのコミット（`git diff --color-moved` で確認できる）と、中身を変えるコミットを混ぜない。
3. **1 ステップ 1 コミット。** 1 コミットは、AI が 1 回の作業で読み切れる差分（目安 1,500 行以下、移動のみなら上限なし）にする。
4. **毎ステップの検証：** `cargo fmt --check`、`cargo clippy --all-targets`（警告数を増やさない。フェーズ 0 以降は `-D warnings`）、`cargo test`。フェーズの区切りでは `cargo llvm-cov` を実行し、カバレッジ 95% 以上を保つ。
5. **未使用コードの削除は事前に一覧で確認する**（「8. 判断が必要な点」）。

## 5. フェーズ別の計画

### フェーズ 0：作業の土台（小）

| ステップ | 内容 | 完了条件 |
|---|---|---|
| 0-1 | `CLAUDE.md` を追加する（「6. AI 向けの作業ガイド」） | AI が最初に読む案内がある |
| 0-2 | 既存の clippy 警告 57 件を解消する。`too_many_arguments` は、この時点では `allow` のまま残す | `cargo clippy --all-targets -- -D warnings` が通る |
| 0-3 | 検証を 1 コマンドにまとめる（`scripts/check.ps1`：fmt・clippy・test） | AI も人も同じ手順で検証できる |
| 0-4 | `#![allow(dead_code)]` を外し、未使用コードを削除する（「8. 判断が必要な点」で合意したもの） | 警告なしでビルドでき、`app_view.rs` が約 300 行減る |

### フェーズ 1：UI に依存しない処理を `core` へ移す（中）

`app_view.rs` 後半のトップレベル関数のうち、egui を使わないものを移動し、ユニットテストも一緒に移します。

| ステップ | 移動するもの | 移動先 |
|---|---|---|
| 1-1 | `EditorTextBuffer`、`LineIndex`、`TextChunk`、位置変換（`char_index_to_position` など）、括弧の対応（`find_matching_bracket_pair` など）、`smart_newline`、`closing_pair` | `core/text/` |
| 1-2 | `CsvDocument`、`parse_csv`、`serialize_csv*`、`CsvTextStyle`、選択範囲のコピー・貼り付け・削除 | `core/csv.rs` |
| 1-3 | `git_command_output`、`parse_git_status`、`parse_git_branches`、差分の解析（`parse_diff_display_rows`、`parse_unified_hunk_header`）、履歴の解析 | `core/git/` |
| 1-4 | `markdown_source_spans` などのソース色分け、定義の収集（D12 で追加した関数）、脚注番号 | `core/markdown/` |
| 1-5 | `NavigationHistory` | `core/navigation.rs` |
| 1-6 | Git を `GitClient` トレイト経由で呼ぶようにする（`Platform` と同じ差し替え方式）。実装は今と同じく `git` コマンドを使う | テストは引き続き本物の git を使える。将来は偽物に差し替えて高速化できる |

完了条件：

- `app_view.rs` が約 4,000 行減る。
- 移動した関数のユニットテストが `core` 側にあり、egui なしで実行できる。

### フェーズ 2：`impl CodeReaderApp` を機能ごとのファイルに分ける（中・機械的）

メソッドを、まとまりごとに `ui/app_view/` の子モジュールへ移します（`impl CodeReaderApp` ブロックを複数ファイルに置く）。フィールドの可視性はそのままです。

| ステップ | 移すメソッド（現在の行番号の目安） | 移動先 |
|---|---|---|
| 2-1 | ファイルツリー・ファイル操作・ワークスペース変更の監視（600〜965） | `workspace/` |
| 2-2 | セッションの作成・切替・名前変更・状態保存（966〜1182） | `workspace/sessions.rs` |
| 2-3 | LSP（1235〜2260） | `lsp.rs` |
| 2-4 | ブックマーク・ナビゲーション（2276〜2440） | `navigation.rs` |
| 2-5 | 検索（2644〜2920、3763〜4195） | `search/` |
| 2-6 | Git（2921〜3762、5207〜5677） | `git/` |
| 2-7 | ダイアログ・Help・ショートカット表示（4196〜5206、5704〜5783） | `windows/`、`keymap.rs` |
| 2-8 | 診断レポート・性能計測（5784〜6160） | `diagnostics.rs` |

完了条件：

- `mod.rs` には `CodeReaderApp` の定義、`new`、`eframe::App::ui` だけが残る。
- 各ファイルが 1,200 行以下になる。

### フェーズ 3：`EditorPane` を中身の種類で分ける（大・最重要）

ここは構造を変えるため、テストでの確認を特に丁寧に行います。

| ステップ | 内容 |
|---|---|
| 3-1 | フィールドをサブ構造体にまとめる（`MarkdownPaneState`、`CsvPaneState`、`VirtualEditorState`、`SourceSearchState`（既存）、`DiffPaneState`、`EditorCaches`）。アクセスは `self.markdown.active_page_id` のように変わるだけで、挙動は変わらない |
| 3-2 | メソッドを `editor/` 以下のファイルへ移す（フェーズ 2 と同じ方式）。`virtual_source_editor_ui`（約 500 行）を入力処理・描画・スクロールバー・IME に分ける |
| 3-3 | ペインの中身を `enum EditorContent { Empty, File(FileView), Diff(DiffView), GitHistory(GitHistoryView), Error(String) }` にする。`FileView` は表示モード（通常・大容量・Markdown・CSV）と、そのモードの状態を持つ。`read_only`、`diff_view.is_none()` などの組み合わせ判定を、`match` に置き換える |
| 3-4 | 長い描画関数を分割する（`csv_ui`、`source_editor_ui`、`markdown_render_node_ui`、`diff_view_ui` など。80 行以下が目安）。引数の多い関数は、`VirtualViewport` などの文脈構造体を受け取る形にする |

完了条件：

- `EditorPane` の直接のフィールドが 15 個以下になる。
- 差分・履歴・空のペインでは、ファイル用の状態が型として存在しない。
- `too_many_arguments` の `allow` がなくなる。

### フェーズ 4：規則を 1 か所にまとめる（中）

| ステップ | 内容 | 防げる不具合の例 |
|---|---|---|
| 4-1 | **キー割り当て表**（`keymap.rs`）：コンテキスト・キー・コマンド・説明を 1 つの表にし、処理、Help、ステータスバーをこの表から作る。修飾キーの照合は 1 つの関数にする（`consume_key` が余分な Shift を無視する問題への対策もここに集める） | D10、D11、D15、F6 の Shift 判定の順序 |
| 4-2 | **手前に出る UI の enum**（`modal.rs`）：Help、Search Everywhere、各ダイアログを `enum Modal` にし、Esc / Enter はフレームの先頭で最前面のものだけに渡す | D02、D15、Unsaved Changes の割り込み |
| 4-3 | **ステータス**（`status.rs`）：ユーザ操作の結果と、バックグラウンド処理の通知（Indexed など）を分けて保持し、操作結果は一定時間優先して表示する | D01 |
| 4-4 | **LSP を言語共通にする**（`lsp.rs`）：Rust と Go で重複している要求・応答・同期・タイムアウト処理を、`LspSession { language, client, pending, ... }` 1 つにまとめる。言語ごとの差（起動コマンド、ウォームアップの有無）は設定値にする | 片方の言語だけ直す漏れ |
| 4-5 | **フォーカス ID**：エディタ・Markdown・CSV のフォーカス ID を、ペインと対象から作る関数に集め、`ui.id()` に依存させない。実在しない ID を要求しないことをテストで保証する（回避コードなしで egui_kittest が通ることが、その保証になる） | D07、D08 |

### フェーズ 5：仕上げ（小）

| ステップ | 内容 |
|---|---|
| 5-1 | 各モジュールの先頭に、責務・主な状態・不変条件を数行で書く（`//!`）。例：「CSV グリッドは移動で表を広げない（D13）」 |
| 5-2 | 診断・性能計測のコードを `diagnostics.rs` に寄せ、描画関数の中では 1 行の記録呼び出しだけにする |
| 5-3 | カバレッジを再計測し、[testing.md](testing.md) の未カバー分類を更新する |
| 5-4 | `CLAUDE.md` の「どこを変えると何が変わるか」を最終の構成に合わせて更新する |

## 6. AI 向けの作業ガイド（`CLAUDE.md` に書く内容）

- **概要**：アプリの目的、対応言語（Rust / Go は LSP、それ以外は表示のみ）。
- **地図**：機能 → ファイルの対応表（「3. 目標の構成」のツリーに、各ファイルの責務を 1 行ずつ付けたもの）。
- **検証**：`scripts/check.ps1`（fmt・clippy・test）。カバレッジは `cargo llvm-cov --summary-only`。
- **テストの書き方**：
  - ユーザ操作で検証する（`TestApp` のヘルパー、1 イベント 1 フレーム、`assert_status` は履歴を見る）。
  - 状態を直接書き換えない。
  - 意図と異なる現状を固定する場合は `DISCREPANCY(Dxx)` を付けて [discrepancies.md](discrepancies.md) に記録する。
- **差し替え点**：
  - OS 依存の処理は `Platform`。
  - LSP は `LspMode`（チャネル・スクリプト・実プロセス）。
  - Git は `GitClient`（フェーズ 1 以降）。
- **egui の注意点**（今回の作業で分かったこと）：
  - `consume_key` は余分な Shift / Alt を無視する。修飾キー付きの判定を先に行う。
  - Esc を受けたフレームでは、そのフレームの開始時点でフォーカスが外れている（`had_focus_last_frame` を使う）。
  - フォーカスは、そのフレームに存在するウィジェットの ID にだけ要求する（AccessKit がパニックする）。
  - 同じ入力でパスが再実行されることがある。トグル操作は冪等でない点に注意する。
- **作業の作法**：
  - 移動と変更を別コミットにする。
  - 1 コミットで 1 つの目的にする。
  - push しない（人が行う）。

## 7. 完了の指標

| 指標 | 現状 | 目標 |
|---|---|---|
| 最大ファイル行数 | 23,796 | 1,200 以下 |
| 100 行超の関数 | 38 | 5 以下（大きな `match` による振り分けだけ） |
| `CodeReaderApp` / `EditorPane` の直接フィールド数 | 69 / 66 | 20 / 15 以下 |
| clippy 警告 | 57 | 0（`-D warnings` で検証） |
| `allow(dead_code)` / `allow(too_many_arguments)` | 1 / 8 | 0 / 0 |
| テスト | 386 件・カバレッジ 95.4% | 全件通過を維持・95% 以上 |
| キー割り当ての定義元 | 3 か所 | 1 か所 |

## 8. 判断が必要な点

着手前に決めておきたい点です。

1. **LSP 非対応言語向けの定義解決（`features/definition.rs`、`core/external_definitions.rs`、`core/external_libraries.rs` の大部分、`core/workspace.rs`）を削除してよいか。** 現在は UI から到達できません。将来 Rust / Go 以外の定義ジャンプを LSP なしで提供する予定があれば、残して到達経路を作る方針に変えます。
2. **診断用のクリック記録・性能計測を、製品に残すか。** 残す場合は `diagnostics.rs` に隔離します。開発時だけで十分なら、cargo の feature（例：`diagnostics`）で切り替えます。
3. **フェーズ 3-3（`EditorContent` enum 化）の範囲。** 最も効果が大きい一方、変更量も最大です。フェーズ 3-1・3-2 までで一度止めて、効果を見てから判断する進め方も可能です。
4. **Git を偽物に差し替えるテストへ移行するか（フェーズ 1-6 の先）。** 現在は本物の git でテストしていて信頼性は高いものの、Git 関連のイベントテスト（33 件）だけで約 30 秒かかります。

## 9. 順序と規模の目安

| フェーズ | 規模 | 依存 |
|---|---|---|
| 0 土台 | 小（コミット 4〜6） | なし。最初に行う |
| 1 core への移動 | 中（6〜8） | 0 |
| 2 CodeReaderApp の分割 | 中（8〜10、機械的） | 0。1 と並行可 |
| 3 EditorPane の分割 | 大（10〜15） | 1・2 の後 |
| 4 規則の一元化 | 中（6〜8） | 2 の後（4-5 は 3 の後） |
| 5 仕上げ | 小（3〜4） | 最後 |

フェーズ 0〜2 は挙動に触れない機械的な作業が中心で、AI に任せやすい部分です。フェーズ 3・4 は設計判断を含むため、ステップごとに差分のレビューを挟むことを推奨します。

## 10. 実施結果

フェーズ 0〜5 を実施しました（`fd20785` 以降のコミット）。判断が必要だった 4 点は、次のとおり決まりました。

- LSP 非対応言語向けの定義解決：削除した。
- 診断用のクリック記録・性能計測：製品に残し、`editor_debug.rs` と `diagnostics.rs` に隔離した。
- `EditorContent` enum 化：実施した（範囲は下記）。
- Git テスト：本物の git のまま。そのためフェーズ 1-6（`GitClient`）は行わなかった。

### 指標

| 指標 | 開始時 | 目標 | 結果 | 達成 |
|---|---|---|---|---|
| 最大ファイル行数（テストを含む） | 23,796 | 1,200 以下 | 1,291（`virtual_editor.rs`。次点は `core/lsp.rs` の 1,223、`diff_view.rs` の 1,215） | **未達** |
| `app_view.rs` | 23,796 | 骨組みのみ | 884 | 達成 |
| 200 行超の関数 | 11 | — | 0 | — |
| 100 行超の関数 | 38 | 5 以下 | 0 | 達成 |
| `CodeReaderApp` / `EditorPane` の直接フィールド数 | 69 / 66 | 20 / 15 以下 | 20 / 17 | ほぼ達成 |
| clippy 警告 | 57 | 0 | 0（`-D warnings` で検証） | 達成 |
| `allow(dead_code)` / `allow(too_many_arguments)` | 1 / 8 | 0 / 0 | 0 / 0 | 達成 |
| テスト | 386 件・95.4% | 全件通過・95% 以上 | 385 件・97.56% | 達成 |
| キー割り当ての定義元 | 3 か所 | 1 か所 | 表示は 1 か所、処理は機能ごと | 一部 |

カバレッジが上がったのは、主に未カバーだった未使用コードを削除したためです。テスト数の増減は、削除したコードのテストを除き、ステータスと Esc のテストを追加した結果です。

### 計画と違う点

- **ファイル構成：** `ui/app_view/` の下にサブフォルダを作らず、35 ファイルをフラットに置いた。子モジュールが `use super::*` と `pub(super)` で親を共有する形は、階層を増やすと可視性の指定が煩雑になるため。
- **`EditorContent`（3-3）：** `path`・`read_only`・差分・履歴を enum にまとめたが、Markdown・CSV・大容量エディタの状態は `File` の中に入れず、ペイン直下の構造体のまま。これらのビューはペインのテキストと一緒に状態を読み書きし、ペインのメソッドを呼び返すため、`File` の中へ移すにはビューの書き直しが必要になる。完了条件の「差分・履歴・空のペインではファイル用の状態が型として存在しない」は満たしていない。
- **キー割り当て（4-1）：** Help とステータスバーは `keymap.rs` の表から作るようにしたが、キーの処理そのものは各機能のまま。処理は egui の入力の消費順と各画面の状態に結びついていて、表から生成すると挙動が変わるおそれがある。処理を変えるときは表の行も直す必要がある。
- **`Modal`（4-2）：** 開いている UI を保持する状態を 1 つの enum にはせず、既存のフィールドから最前面を求める `top_dialog()` にした。優先順位と Esc の配分は 1 か所になったが、複数のダイアログが同時に開ける状態自体は残っている。
- **長い関数（3-4）：** 100 行超は 0 になったが、80 行超は 41 個残っている。関数を分けた分だけ 3 ファイルが 1,200 行を超えた。
