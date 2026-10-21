# ユーザ操作の意図と実際の挙動の乖離（修正方針の提案）

ユーザイベント起因テスト（`src/ui/app_view/event_tests/`）で見つかった、ユーザ操作の意図と実際の処理が食い違う箇所の一覧と、プロダクトコードの修正方針です。

- 各項目は、現状の挙動をそのままアサーションしたテストで固定しています。テストには `DISCREPANCY(Dxx)` のコメントがあります。
- 修正するときは、該当テストのアサーションを「意図どおりの挙動」に反転させてから実装してください（テストが先に赤くなり、修正で緑に戻る順序）。
- 優先度：**高** = データ消失・誤操作の危険がある / **中** = 案内どおりに操作できない / **低** = 表示や細かな使い勝手。

## 一覧

| ID | 優先度 | 概要 | 固定しているテスト |
|---|---|---|---|
| D06 | 高 | 未保存の編集が、別ファイルを開くと警告なく消える | `editor.rs` `opening_another_file_discards_unsaved_edits_without_warning` |
| D03 | 高 | Shift二度押し判定が Ctrl+Shift 系や Shift+Enter の連打でも発火し、ファイルの再読込（未保存編集の消失）まで起きる | `editor.rs` `quick_shift_enter_presses_reopen_the_file_through_search_everywhere` / `windows.rs` `two_quick_shift_combinations_open_search_everywhere` |
| D16 | 高 | 大容量ファイルをツリーから開いた直後、エディタをクリックしていなくても入力がファイル先頭に入る。Delete だけはファイル削除ダイアログになる | `virtual_editor.rs` `typing_right_after_opening_from_the_tree_edits_the_top_of_the_file` |
| D13 | 高 | CSV で最終列を越えて移動しただけで空の列が追加され、保存される | `csv.rs` `navigating_past_the_edge_adds_rows_and_columns_to_the_file` |
| D04 | 高 | 空白・日本語を含むファイル名で Git の stage / diff / discard が失敗する | `git.rs` `files_with_spaces_or_japanese_names_cannot_be_staged` |
| D07 | 高（要実機確認） | 実在しないウィジェットIDにフォーカスを置いており、スクリーンリーダー等のアクセシビリティ機能が有効だとパニックし得る | テスト用回避コード `register_focused_test_node`（`app_view.rs`） |
| D10 | 中 | Ctrl+Shift+Z（ヘルプ上は Redo）が Undo として動く（大容量エディタ・CSVグリッド） | `virtual_editor.rs` `clicking_places_the_cursor_and_typing_edits` / `csv.rs` `ctrl_shift_z_undoes_in_the_grid` |
| D08 | 中 | 大容量ファイルで Tab を押すとスペースが入らずフォーカスが移り、以後の入力が効かない | `virtual_editor.rs` `tab_moves_keyboard_focus_away_from_the_large_file_editor` |
| D02 | 中 | 各検索ボックスで Esc が効かない（ヘルプには「Esc: Clear/Close」） | `search.rs` `escape_in_workspace_search_clears_it` / `editor.rs` `find_without_matches_and_escape` / `markdown.rs` `rendered_search_finds_text_across_pages` |
| D15 | 中 | Switch Branch / Discard Changes ウィンドウで Enter が効かない（ヘルプとステータスバーには「Enter: Confirm」） | `windows.rs` `help_and_status_bar_follow_the_current_context` |
| D05 | 中 | Commit & Push が upstream 未設定のブランチを公開しない（コミットだけ成功して push は失敗） | `git.rs` `commit_and_push_without_upstream_commits_but_fails_to_push` |
| D11 | 中 | ヘルプにある Markdown Pages / Outline のキー操作（Up/Down/Enter）に到達する手段がない | `markdown.rs` `pages_and_outline_keyboard_navigation_cannot_be_reached` |
| D12 | 中 | 脚注・参照リンクの定義が別ページ（通常は文書末尾）にあると、リンクにならず文字のまま表示される | `markdown.rs` `preview_renders_the_first_page_with_navigation` |
| D09 | 低 | 大容量ファイルでホイール1回のスクロール量が約1.7倍になる | `virtual_editor.rs` `one_wheel_event_keeps_scrolling_on_later_frames` |
| D14 | 低 | CSV グリッドで1セル編集すると、ファイル末尾の改行が消える | `csv.rs` `grid_shows_cells_and_edits_the_selected_one` |
| D01 | 低 | ルートフォルダ削除時の「Removed folder」メッセージが即座に上書きされ表示されない | `workspace.rs` `add_folder_adds_a_root_and_remove_button_drops_it` |

---

## 詳細と修正方針

### D06 未保存の編集が警告なく消える（高）
- **意図**: 編集中のファイルから別ファイルへ移っても、保存していない変更は失われない（少なくとも確認される）。
- **実際**: `EditorPane` に「未保存」状態がなく、`open_file_in_pane` / `open_navigation_point_in_focused_pane` / Git の差分表示などが `*self.editor_mut(pane) = next` で編集内容ごと置き換える。
- **修正方針**:
  1. `EditorPane` に保存時点の内容のフィンガープリント（`text_fingerprint()` の値）を持たせ、`is_dirty()` を追加する。`save_to_disk` 成功時と `from_path` 時に更新する。
  2. ペインの内容を置き換える処理を 1 つの関数（例：`replace_focused_editor(next)`）に集約し、dirty なら「保存 / 破棄 / キャンセル」の確認ウィンドウを出す。
  3. タイトルに未保存マーク（`●` など）を表示する。
- **影響範囲**: ファイルツリーのクリック、Search Everywhere、定義ジャンプ、Back/Forward、Git の差分・履歴表示、セッション切替、ファイルのリネーム・削除。

### D03 Shift二度押しの誤検出（高）
- **意図**: Shift だけを素早く2回押したときに Search Everywhere を開く。
- **実際**: `consume_search_shortcuts` は「Shift 修飾キーが押下状態になった瞬間」だけを数えるため、Ctrl+Shift+A / Ctrl+Shift+Z / Shift+Enter / Shift+矢印 を 0.45 秒以内に2回行っても発火する。さらに同じフレームの Enter を開いたばかりのオーバーレイが受け取り、先頭候補（多くは今開いているファイル）を開き直す。このためファイルがディスクから再読込され、未保存の編集が失われる。
- **修正方針**: Shift の押下から解放までの間に、Shift 以外のキーイベント（`Event::Key` / `Event::Text`）や他の修飾キーが一度でもあれば、その押下は「単独タップ」として数えない。開いたフレームでは Enter / 矢印を消費しない（オーバーレイ側で「開いた直後のフレーム」を無視する）。

### D16 クリック前の入力が大容量ファイルの先頭に入る（高）
- **意図**: エディタをクリックして（またはフォーカスして）から入力が入る。
- **実際**: 仮想エディタは「どのウィジェットもフォーカスしていない」ときにキー入力を受け付け、カーソルが未設定だと位置 (0,0) に挿入する。一方 Delete は、ファイルツリーがアクティブなままなのでファイル削除ダイアログになり、挙動が一貫しない。
- **修正方針**: D08 の修正（仮想エディタを実ウィジェットにしてフォーカスを持たせる）とあわせて、`virtual_editor_can_handle_key` を「エディタ自身がフォーカスを持つ場合のみ」に限定する。ファイルを開いたときのフォーカス移動の方針（ツリーに残す / エディタに移す）を決め、Tab / Delete / 文字入力で同じ規則に従わせる。

### D13 CSV の移動だけで列・行が増える（高）
- **意図**: 矢印・Tab・Enter はセル間の移動であり、データは変えない。
- **実際**: `csv_navigation_target` が範囲外（最終列の右・最終行の下）を返し、`csv_ui` が `ensure_cell` で空セルを追加する。保存時に末尾の空行は削られるが、空の列は全行に付いたまま保存される。
- **修正方針**: 移動先は既存範囲にクランプする。行・列の追加は明示操作（例：最終行で Enter を押したときだけ行を追加、列追加はメニュー）に限定する。`csv_navigation_target` には同じ内容の分岐が重複しているので、あわせて整理する。

### D04 空白・日本語ファイル名での Git 操作失敗（高）
- **意図**: どのファイル名でも stage / unstage / diff / discard できる。
- **実際**: `git status --porcelain=v1`（`-z` なし）は特殊文字を含むパスを `"..."` で囲み、非 ASCII を `\346...` の 8 進エスケープで出力する。`parse_git_status` はそれをそのままパスとして使うため、後続の `git add -- <path>` などが失敗する。ツリーの変更マークも付かない。
- **修正方針**: `git status --porcelain=v1 -z -uall` に変更し、NUL 区切りで解析する。リネーム（`R  old -> new`）は `-z` では `new\0old\0` の順になるので、`GitChange` に `old_path` を持たせる。`git_change_workspace_path` の `" -> "` 分割は不要になる。

### D07 実在しないウィジェットへのフォーカス（高・要実機確認）
- **意図**: キーボードフォーカスは画面上のウィジェットに置かれる。
- **実際**: 仮想エディタは `text_edit_id`、Markdown の各ペインは `markdown_focus_id(ui, target)` にフォーカスを要求するが、どちらも対応するウィジェットがない（Markdown 側は呼び出し元の `ui.id()` から作るため、本文ウィジェットの ID とも一致しない）。AccessKit が有効だと、フォーカス中の ID に対応するノードがないことで `accesskit_consumer` がパニックする。テストでは常に AccessKit が有効なため、テストビルド限定の `register_focused_test_node` で回避している。実アプリでもスクリーンリーダーなどの UI Automation クライアントが AccessKit を有効化すると、同じパニックが起きる可能性が高い（未検証）。
- **修正方針**: 仮想エディタの描画領域を `ui.interact(viewport_rect, text_edit_id, Sense::click_and_drag())` で実ウィジェットにし、`memory.set_focus_lock_filter` で矢印・Tab を自分に留める（D08・D16 も同時に解消）。Markdown のフォーカス要求は本文ウィジェットの ID（`body_id`）に統一する。修正後は `register_focused_test_node` を削除する。

### D10 Ctrl+Shift+Z が Undo になる（中）
- **意図**: Ctrl+Shift+Z は Redo（README とヘルプの記載どおり）。
- **実際**: `consume_undo_shortcut` の末尾で `input.consume_key(Modifiers::CTRL, Key::Z)` を呼んでいる。egui の `consume_key` は余分な Shift を無視して照合するため、Ctrl+Shift+Z がここで Undo として消費される。Redo 側の判定（`consume_redo_shortcut` の Shift 付き分岐）には到達しない。
- **修正方針**: Redo を Undo より先に判定する。あるいは Undo 側の `consume_key` フォールバックを削除し、修飾キーを厳密に照合する `retain` ベースの判定だけにする。

### D08 大容量ファイルで Tab がフォーカスを奪う（中）
- **意図**: Tab でスペース（インデント）を挿入する。
- **実際**: 仮想エディタは何もフォーカスしていない状態でキーを受けるため、egui が Tab を「次のウィジェットへフォーカス移動」と解釈し、先に描画される上部バーのボタンにフォーカスが移る。以後 `virtual_editor_can_handle_key` が false になり、入力が効かない。
- **修正方針**: D07 の修正（実ウィジェット化とフォーカスロック）で解消する。

### D02 検索ボックスで Esc が効かない（中）
- **意図**: Esc で検索ボックスを閉じる / クリアする（ヘルプとステータスバーの表示どおり）。
- **実際**: egui は Esc を受けたフレームの開始時（`Memory::begin_pass`）にフォーカスを外す。`workspace_search_ui` / `source_search_ui` / `markdown_search_ui` は「直前フレームでフォーカスがあったか」を条件に Esc を見ているため、条件が常に偽になる。
- **修正方針**: フォーカスの有無を問わず、検索 UI が開いていてそのフレームに Esc（修飾なし）があれば閉じる。あるいは TextEdit に `EventFilter { escape: true, .. }` のフォーカスロックを設定し、TextEdit 自身で Esc を受け取る。

### D15 Switch Branch / Discard Changes で Enter が効かない（中）
- **意図**: Enter で確定する（ヘルプとステータスバーの表示どおり）。
- **実際**: `show_git_switch_branch_window` と `show_git_discard_window` は Escape しか処理していない。
- **修正方針**: Discard は Enter で「Discard」を実行する（破壊的操作なので、既定ボタンを明示する）。Switch Branch は一覧に選択状態（上下キー）を持たせて Enter で切替えるか、それをしないならヘルプの「Enter: Confirm」を削除する。

### D05 Commit & Push が未公開ブランチを公開しない（中）
- **意図**: 「Commit & Push」でコミットをリモートへ反映する。
- **実際**: `commit_git_repository` は常に `git push` を実行し、upstream 未設定なら失敗する。メニューの「Publish Branch」（`push -u <remote> <branch>`）とは別の処理になっている。
- **修正方針**: push 部分を `push_git_repository` に委譲し、upstream がなければ Publish と同じ処理にする。

### D11 Markdown Pages / Outline のキー操作に到達できない（中）
- **意図**: ヘルプにあるとおり、Pages / Outline ペインで Up/Down/Enter で移動できる。
- **実際**: `request_markdown_focus` は Body 指定でしか呼ばれず、Pages / Outline へフォーカスを移す操作がない。`handle_markdown_preview_navigation` の該当分岐と、ヘルプの「Markdown Pages / Markdown Outline」コンテキストは到達不能。
- **修正方針**: ペイン見出しのクリックや、ペイン間を移動するショートカット（例：Ctrl+Tab / F6）で `markdown_focus_target` を切り替えられるようにする。不要な機能であれば、キー処理とヘルプ項目を削除する。

### D12 別ページの脚注・参照リンク定義が解決されない（中）
- **意図**: 文書内のどこかに定義があれば、脚注 `[^x]` と参照リンク `[text][ref]` がリンクとして表示される。
- **実際**: プレビューはページ（H1/H2 区切り）や見出し区間の断片ごとに `markdown::to_mdast` でパースし直すため、別の断片にある定義を参照できず文字のまま表示される。脚注の一覧も、定義が書かれたページにしか出ない。定義が同じ断片にある参照リンクも「Reference links are not resolved yet」となり、リンク先へは移動しない。
- **修正方針**: 文書全体の AST（`MarkdownDocument::ast`）から定義（`Definition` / `FootnoteDefinition`）を集めて表を作り、断片の描画時に参照する。断片パースをやめ、全体 AST のノードを位置で切り出して描画する形にすると根本的に解決する。

### D09 ホイールの過剰スクロール（低）
- **意図**: ホイール1回分だけスクロールする。
- **実際**: 仮想エディタは生のホイール量を即時に適用し、そのフレームの `smooth_scroll_delta` だけを 0 にしている。egui 内部に残った平滑化分が後続フレームで再び届くため、100px のホイールが最終的に約 168px になる。カーソル移動直後の表示位置もずれる。
- **修正方針**: 即時適用と egui の平滑化のどちらかに統一する。即時適用を続けるなら、ホイールイベントを `input.events` から取り除き、egui 側に平滑化させない。

### D14 CSV 編集で末尾改行が消える（低）
- **意図**: 1セルの変更は、その部分だけがファイル差分になる。
- **実際**: `serialize_csv` / `serialize_csv_for_save` は行を `"\n"` で連結するだけで、元の末尾改行を保持しない。改行コード（CRLF）も LF に変わる。
- **修正方針**: 読込時に「末尾改行の有無」と「改行コード」を記録し、シリアライズ時に復元する。

### D01 ルートフォルダ削除メッセージが上書きされる（低）
- **意図**: 「Removed folder: …」が表示される。
- **実際**: `remove_root_from_current_session` がメッセージを設定した後に `rebuild_file_tree()` を呼び、そこで「Discovered N files.」に上書きされる（追加時は順序が逆で、正しく表示される）。
- **修正方針**: `rebuild_file_tree()` の後にメッセージを設定する。より一般には、ユーザ操作の結果メッセージとバックグラウンド処理の通知（「Indexed N workspace symbols.」など）を別枠にし、操作結果が数フレームで消えないようにする。

---

## コードレビューで見つかった事項（テストでの固定なし・参考）

| ID | 概要 | 方針 |
|---|---|---|
| R01 | `file_uri_to_path` が UNC 形式の URI（`file://server/share/...`、`path_to_file_uri` が UNC パスに対して生成する形式）を相対パスに変換してしまう。ネットワーク共有上のワークスペースで定義ジャンプ先を開けない可能性がある | 往復変換のテストを追加し、authority 部を `\\server\share` に戻す |
| R02 | 状態ファイルの `current_session_id` が存在しないセッションを指すと、画面にはフォールバックの Default が表示される。しかし「Add」は成功メッセージを出すだけで何も保存されず、ツリーも空になる（`workspace.rs` `inconsistent_state_without_a_current_session` で現状を確認済み） | 読込時に不整合を検出し、先頭セッションに切り替えるか新規セッションを作って保存する |
| R03 | Markdown ソースモードで見出しの色付けが効かない。見出し全体のスパンが、`#` 記号部分のスパンと重なってソート時に捨てられる（`markdown_source_text_format` の Heading 分岐は到達不能） | 見出しは記号部分とテキスト部分の2スパンに分けて生成する |
| R04 | 定義ジャンプの履歴は4件に制限されるため、パンくずの「...」表示（古い履歴の省略表示）は出ない | 上限と表示件数のどちらかを見直す。または省略表示を削除する |
| R05 | 仮想エディタのクリック・ドラッグ判定は、スクロールバー除外のための当たり判定に「表示領域の大きさ」を全体サイズとして渡しているため、除外判定が常に無効になっている。表示領域がスクロールバーを除いた範囲なので結果的には動作している | 除外ロジックを削除するか、本来の全体サイズを渡す |
| R06 | 大容量ファイル（仮想エディタ）では括弧の自動補完がなく、上下移動で列位置を保持しない（通常エディタとの不一致） | 入力補助を共通化する |
| R07 | 仮想エディタは Ctrl 修飾付きの Text イベントを捨てるため、AltGr（Windows では Ctrl+Alt として届く）で入力する文字（欧州配列の `@` など）が入らない可能性がある | AltGr（Ctrl+Alt 同時）の場合は受け付ける |
| R08 | 検索ボックスを開いたまま Ctrl+F を押すと、現在の一致位置が先頭に戻る | 既に開いていれば位置を保持する |
| R09 | 差分ビューの「◇ 非表示 N 行」だけ日本語で、他の UI は英語 | 表記を統一する |
