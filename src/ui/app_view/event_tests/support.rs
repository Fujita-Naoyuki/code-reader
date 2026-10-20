//! Shared fixtures for user-event tests.
//!
//! [`TestApp`] boots the real [`CodeReaderApp`] inside an `egui_kittest` harness, on a
//! temporary workspace, with a [`FakePlatform`] installed. Tests then drive it the way a
//! user would: clicking widgets by label, pressing keys, typing text and waiting for
//! background work (indexing, search, Git) to land.

use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
    time::{Duration, Instant},
};

use eframe::egui;
use egui::accesskit;
use egui_kittest::{
    Harness,
    kittest::{By, NodeT, Queryable},
};

use crate::core::location::{Location, Position, Range};
use crate::core::lsp::{
    LspClient, LspClientEvent, LspServerConfig, LspServerEvent, LspServerEventKind, LspTestRequest,
    LspTestServer,
};
use crate::languages::{GoLanguage, LanguageRegistry, RustLanguage};
use crate::platform::{self, Platform, PlatformGuard};

use super::super::CodeReaderApp;

/// Ctrl as egui-winit reports it on Windows: both `ctrl` and the logical `command` are set.
pub(crate) const CTRL: egui::Modifiers = egui::Modifiers {
    alt: false,
    ctrl: true,
    shift: false,
    mac_cmd: false,
    command: true,
};

/// Scriptable stand-in for the OS. Every field can be inspected or changed from a test.
pub(crate) struct FakePlatform {
    pub state_path: PathBuf,
    pub current_dir: PathBuf,
    /// `None` means the clipboard is empty / unreadable.
    pub clipboard: RefCell<Option<String>>,
    pub clipboard_fails: Cell<bool>,
    /// Answers for the next folder pickers, in order. An empty queue means "canceled".
    pub folder_picks: RefCell<VecDeque<PathBuf>>,
    pub picker_titles: RefCell<Vec<String>>,
    pub opened_urls: RefCell<Vec<String>>,
    pub open_url_fails: Cell<bool>,
    pub lsp_servers: RefCell<Vec<FakeLspServer>>,
}

pub(crate) struct FakeLspServer {
    pub config: LspServerConfig,
    pub server: LspTestServer,
}

impl FakePlatform {
    fn new(state_path: PathBuf, current_dir: PathBuf) -> Self {
        Self {
            state_path,
            current_dir,
            clipboard: RefCell::new(None),
            clipboard_fails: Cell::new(false),
            folder_picks: RefCell::new(VecDeque::new()),
            picker_titles: RefCell::new(Vec::new()),
            opened_urls: RefCell::new(Vec::new()),
            open_url_fails: Cell::new(false),
            lsp_servers: RefCell::new(Vec::new()),
        }
    }

    pub fn clipboard_text(&self) -> Option<String> {
        self.clipboard.borrow().clone()
    }

    pub fn set_clipboard(&self, text: &str) {
        *self.clipboard.borrow_mut() = Some(text.to_owned());
    }

    pub fn queue_folder_pick(&self, path: &Path) {
        self.folder_picks.borrow_mut().push_back(path.to_path_buf());
    }

    pub fn lsp_server_names(&self) -> Vec<String> {
        self.lsp_servers
            .borrow()
            .iter()
            .map(|server| server.config.server_name.clone())
            .collect()
    }

    /// Drains the requests sent to the most recently started server with `server_name`.
    pub fn lsp_requests(&self, server_name: &str) -> Vec<LspTestRequest> {
        self.lsp_servers
            .borrow()
            .iter()
            .rev()
            .find(|server| server.config.server_name == server_name)
            .map(|server| server.server.take_requests())
            .unwrap_or_default()
    }

    pub fn lsp_send(&self, server_name: &str, event: LspClientEvent) {
        if let Some(server) = self
            .lsp_servers
            .borrow()
            .iter()
            .rev()
            .find(|server| server.config.server_name == server_name)
        {
            server.server.send(event);
        }
    }

    pub fn lsp_respond(
        &self,
        server_name: &str,
        request: &LspTestRequest,
        result: Result<Vec<Location>, String>,
    ) {
        if let Some(server) = self
            .lsp_servers
            .borrow()
            .iter()
            .rev()
            .find(|server| server.config.server_name == server_name)
        {
            server.server.respond(request, result);
        }
    }

    /// Simulates the LSP worker process dying: later requests fail to queue.
    pub fn stop_lsp_servers(&self) {
        self.lsp_servers.borrow_mut().clear();
    }
}

impl Platform for FakePlatform {
    fn state_file_path(&self) -> PathBuf {
        self.state_path.clone()
    }

    fn current_dir(&self) -> PathBuf {
        self.current_dir.clone()
    }

    fn copy_text(&self, text: &str) -> Result<(), String> {
        if self.clipboard_fails.get() {
            return Err("clipboard unavailable".to_owned());
        }
        *self.clipboard.borrow_mut() = Some(text.to_owned());
        Ok(())
    }

    fn read_text(&self) -> Result<String, String> {
        if self.clipboard_fails.get() {
            return Err("clipboard unavailable".to_owned());
        }
        self.clipboard
            .borrow()
            .clone()
            .ok_or_else(|| "clipboard is empty".to_owned())
    }

    fn pick_folder(&self, title: &str) -> Option<PathBuf> {
        self.picker_titles.borrow_mut().push(title.to_owned());
        self.folder_picks.borrow_mut().pop_front()
    }

    fn open_url(&self, url: &str) -> Result<(), String> {
        if self.open_url_fails.get() {
            return Err("no browser".to_owned());
        }
        self.opened_urls.borrow_mut().push(url.to_owned());
        Ok(())
    }

    fn start_lsp(&self, config: LspServerConfig) -> LspClient {
        let (client, server) = LspClient::in_memory(&config.server_name);
        self.lsp_servers
            .borrow_mut()
            .push(FakeLspServer { config, server });
        client
    }
}

pub(crate) struct TestAppBuilder {
    files: Vec<(String, Vec<u8>)>,
    state_json: Option<String>,
    size: egui::Vec2,
    git: bool,
}

impl TestAppBuilder {
    pub fn file(mut self, relative_path: &str, content: &str) -> Self {
        self.files
            .push((relative_path.to_owned(), content.as_bytes().to_vec()));
        self
    }

    pub fn bytes(mut self, relative_path: &str, content: &[u8]) -> Self {
        self.files
            .push((relative_path.to_owned(), content.to_vec()));
        self
    }

    /// Persisted state to start from. `{root}` is replaced with the workspace root and
    /// `{tmp}` with the temporary directory that holds it (both JSON-escaped).
    pub fn state(mut self, json: &str) -> Self {
        self.state_json = Some(json.to_owned());
        self
    }

    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.size = egui::vec2(width, height);
        self
    }

    /// Turns the workspace into a Git repository with one initial commit of all files.
    pub fn git(mut self) -> Self {
        self.git = true;
        self
    }

    pub fn build(self) -> TestApp {
        let temp_dir = tempfile::Builder::new()
            .prefix("code-reader-test-")
            .tempdir()
            .expect("create temp dir");
        let tmp = temp_dir.path().to_path_buf();
        let root = tmp.join("workspace");
        fs::create_dir_all(&root).expect("create workspace root");
        for (relative_path, content) in &self.files {
            let path = root.join(relative_path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent dir");
            }
            fs::write(&path, content).expect("write workspace file");
        }
        if self.git {
            init_git_repository(&root);
        }

        let state_path = tmp.join("state").join("state.json");
        if let Some(json) = &self.state_json {
            fs::create_dir_all(state_path.parent().unwrap()).unwrap();
            let json = json
                .replace("{root}", &json_escape_path(&root))
                .replace("{tmp}", &json_escape_path(&tmp));
            fs::write(&state_path, json).unwrap();
        }

        let platform = Rc::new(FakePlatform::new(state_path, root.clone()));
        let guard = platform::install(platform.clone());
        let registry =
            LanguageRegistry::with_languages(vec![Box::new(RustLanguage), Box::new(GoLanguage)]);
        let harness = Harness::builder()
            .with_size(self.size)
            .with_max_steps(10_000)
            .with_step_dt(1.0 / 60.0)
            .build_eframe(move |creation_context| CodeReaderApp::new(creation_context, registry));

        let mut app = TestApp {
            harness,
            platform,
            root,
            tmp,
            statuses: Vec::new(),
            _guard: guard,
            _temp_dir: temp_dir,
        };
        app.step();
        app.wait_for_indexing();
        app.clear_statuses();
        app
    }
}

pub(crate) struct TestApp {
    pub harness: Harness<'static, CodeReaderApp>,
    pub platform: Rc<FakePlatform>,
    pub root: PathBuf,
    pub tmp: PathBuf,
    /// Every distinct status-bar message seen so far, oldest first. Background work
    /// (indexing, scans) often replaces a message within a frame or two, so tests assert
    /// on this history instead of only the latest message.
    pub statuses: Vec<String>,
    _guard: PlatformGuard,
    _temp_dir: tempfile::TempDir,
}

impl TestApp {
    pub fn builder() -> TestAppBuilder {
        TestAppBuilder {
            files: Vec::new(),
            state_json: None,
            size: egui::vec2(1500.0, 820.0),
            git: false,
        }
    }

    /// Workspace with a couple of plain text files and no language servers.
    pub fn plain() -> Self {
        Self::builder()
            .file("notes.txt", "alpha\nbeta\ngamma\n")
            .file("docs/guide.txt", "guide line\n")
            .build()
    }

    /// Opens a floating window by setting state directly and lets it settle: egui lays a new
    /// window out invisibly on its first frame, so clicks need a few frames to land.
    pub fn settle(&mut self) {
        self.steps(3);
    }

    pub fn app(&self) -> &CodeReaderApp {
        self.harness.state()
    }

    pub fn app_mut(&mut self) -> &mut CodeReaderApp {
        self.harness.state_mut()
    }

    /// Absolute path of a workspace file; `/` separators are converted for the platform.
    pub fn path(&self, relative_path: &str) -> PathBuf {
        self.root
            .join(relative_path.replace('/', std::path::MAIN_SEPARATOR_STR))
    }

    pub fn status(&self) -> String {
        self.app().status_message.clone()
    }

    pub fn step(&mut self) {
        self.harness.step();
        self.record_status();
    }

    fn record_status(&mut self) {
        let status = self.harness.state().status_message.clone();
        if self.statuses.last() != Some(&status) {
            self.statuses.push(status);
        }
    }

    /// True if any status message seen so far equals `message`.
    pub fn saw_status(&self, message: &str) -> bool {
        self.statuses.iter().any(|status| status == message)
    }

    /// True if any status message seen so far contains `text`.
    pub fn saw_status_containing(&self, text: &str) -> bool {
        self.statuses.iter().any(|status| status.contains(text))
    }

    #[track_caller]
    pub fn assert_status(&self, message: &str) {
        assert!(
            self.saw_status(message),
            "status {message:?} not seen; history: {:#?}",
            self.statuses
        );
    }

    #[track_caller]
    pub fn assert_status_containing(&self, text: &str) {
        assert!(
            self.saw_status_containing(text),
            "no status containing {text:?}; history: {:#?}",
            self.statuses
        );
    }

    /// The status bar text (status message plus rust-analyzer readiness) is painted, not a
    /// widget, so read it from the app.
    pub fn status_bar_contains(&self, text: &str) -> bool {
        self.app().status_bar_message().contains(text)
    }

    pub fn clear_statuses(&mut self) {
        self.statuses.clear();
    }

    pub fn steps(&mut self, count: usize) {
        for _ in 0..count {
            self.step();
        }
    }

    /// Runs frames until `condition` holds, failing the test after a few seconds.
    pub fn wait_until(&mut self, what: &str, condition: impl Fn(&CodeReaderApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !condition(self.app()) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; statuses: {:#?}",
                self.statuses
            );
            self.step();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Runs frames until a status message containing `text` has been shown.
    pub fn wait_for_status(&mut self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.saw_status_containing(text) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for status {text:?}; statuses: {:#?}",
                self.statuses
            );
            self.step();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Waits for the initial background symbol indexing to finish.
    pub fn wait_for_indexing(&mut self) {
        self.wait_until("symbol indexing", |app| app.symbol_index_receiver.is_none());
    }

    pub fn has_label(&self, label: &str) -> bool {
        self.harness.query_all_by_label(label).next().is_some()
    }

    pub fn has_label_containing(&self, text: &str) -> bool {
        self.harness
            .query_all_by_label_contains(text)
            .next()
            .is_some()
    }

    pub fn count_label(&self, label: &str) -> usize {
        self.harness.query_all_by_label(label).count()
    }

    pub fn nth_label_rect(&self, label: &str, index: usize) -> egui::Rect {
        self.harness
            .query_all_by_label(label)
            .nth(index)
            .unwrap_or_else(|| panic!("no widget #{index} labeled {label:?}"))
            .rect()
    }

    // Input helpers queue exactly one egui event per frame, so the status history above
    // sees every intermediate message (kittest would otherwise run several frames inside
    // one `step()` call).

    fn send(&mut self, event: egui::Event) {
        self.harness.event(event);
        self.step();
    }

    pub fn click(&mut self, label: &str) {
        assert_eq!(self.count_label(label), 1, "expected exactly one {label:?}");
        self.click_nth(label, 0);
    }

    /// Clicks the first widget labeled `label` in tree order. The sidebar is laid out before
    /// the editors and floating windows, so this picks a file-tree entry over an editor
    /// title with the same text, or a sidebar button over a dialog title.
    pub fn click_first(&mut self, label: &str) {
        self.click_nth(label, 0);
    }

    /// Clicks the `index`-th widget (in tree order) whose label is exactly `label`,
    /// scrolling it into view first.
    pub fn click_nth(&mut self, label: &str, index: usize) {
        let count = self.count_label(label);
        assert!(
            count > index,
            "expected at least {} widgets labeled {label:?}, found {count}",
            index + 1,
        );
        self.scroll_into_view(label, index);
        let rect = self.nth_label_rect(label, index);
        self.click_at(rect.center());
    }

    /// Asks egui (through AccessKit) to scroll the `index`-th widget labeled `label` into
    /// view, if it is outside the visible area.
    pub fn scroll_into_view(&mut self, label: &str, index: usize) {
        // Keep clear of the top bar and the status bar, which cover the panels' edges.
        let visible = self
            .harness
            .ctx
            .content_rect()
            .shrink2(egui::vec2(0.0, 40.0));
        let rect = self.nth_label_rect(label, index);
        if visible.contains_rect(rect) {
            return;
        }
        if let Some(node) = self.harness.query_all_by_label(label).nth(index) {
            node.scroll_to_me();
        }
        self.steps(3);
    }

    /// Clicks a text-like widget (label, link, selectable label) by its exact text; when the
    /// text appears several times the first one in tree order is used.
    pub fn click_text_label(&mut self, label: &str) {
        self.click_first(label);
    }

    /// Re-focuses the rendered Markdown search box with Ctrl+F and replaces its query.
    pub fn fill_markdown_search(&mut self, text: &str) {
        self.ctrl(egui::Key::F);
        self.ctrl(egui::Key::A);
        self.type_text(text);
    }

    /// Debug helper: every node label on screen.
    pub fn all_labels(&self) -> Vec<String> {
        self.harness
            .query_all(By::new().predicate(|_| true))
            .filter_map(|node| node.accesskit_node().label())
            .collect()
    }

    pub fn click_containing(&mut self, text: &str) {
        let visible = self
            .harness
            .ctx
            .content_rect()
            .shrink2(egui::vec2(0.0, 40.0));
        if !visible.contains_rect(self.harness.get_by_label_contains(text).rect()) {
            self.harness.get_by_label_contains(text).scroll_to_me();
            self.steps(3);
        }
        let rect = self.harness.get_by_label_contains(text).rect();
        self.click_at(rect.center());
    }

    pub fn right_click(&mut self, label: &str) {
        let rect = self.harness.get_by_label(label).rect();
        self.secondary_click_at(rect.center());
    }

    pub fn hover(&mut self, label: &str) {
        let rect = self.harness.get_by_label(label).rect();
        self.move_pointer(rect.center());
        self.step();
    }

    pub fn label_rect(&self, label: &str) -> egui::Rect {
        self.harness.get_by_label(label).rect()
    }

    /// Rects of every text field on screen, in tree order.
    pub fn text_field_rects(&self) -> Vec<egui::Rect> {
        self.harness
            .query_all(By::new().predicate(|node| {
                matches!(
                    node.role(),
                    accesskit::Role::TextInput | accesskit::Role::MultilineTextInput
                )
            }))
            .map(|node| node.rect())
            .collect()
    }

    /// Rect of the main editor text area (the largest multi-line text field on screen).
    pub fn editor_text_rect(&self) -> egui::Rect {
        self.harness
            .query_all(
                By::new().predicate(|node| node.role() == accesskit::Role::MultilineTextInput),
            )
            .map(|node| node.rect())
            .max_by(|a, b| a.area().total_cmp(&b.area()))
            .expect("an editor text area")
    }

    /// Monospace editor metrics: (row height, character width).
    pub fn editor_metrics(&self) -> (f32, f32) {
        let font = egui::FontId::monospace(14.0);
        self.harness
            .ctx
            .fonts_mut(|fonts| (fonts.row_height(&font), fonts.glyph_width(&font, 'x')))
    }

    /// Screen position of `(line, column)` in the editor text area (0-based, no scroll).
    pub fn editor_text_pos(&self, line: u32, column: u32) -> egui::Pos2 {
        let rect = self.editor_text_rect();
        let (row_height, char_width) = self.editor_metrics();
        egui::pos2(
            rect.left() + column as f32 * char_width + char_width * 0.3,
            rect.top() + 2.0 + line as f32 * row_height + row_height * 0.5,
        )
    }

    /// Clicks in the editor so the cursor lands at `(line, column)`.
    pub fn click_text(&mut self, line: u32, column: u32) {
        // Let egui's double-click window pass so repeated clicks do not select a word.
        self.steps(25);
        let pos = self.editor_text_pos(line, column);
        self.click_at(pos);
    }

    /// Geometry of the focused virtual (large-file) editor. It is painted rather than built
    /// from widgets, so it is read from the click diagnostics the editor records; a
    /// harmless click on the pane title produces such a record when none exists yet.
    pub fn virtual_geometry(&mut self) -> VirtualGeometry {
        if self
            .app()
            .focused_editor()
            .virtual_click_debug
            .events
            .is_empty()
        {
            let title = self.app().focused_editor().title.clone();
            let count = self.count_label(&title);
            let rect = self.nth_label_rect(&title, count - 1);
            self.click_at(rect.center());
        }
        let editor = self.app().focused_editor();
        let event = editor
            .virtual_click_debug
            .events
            .last()
            .expect("virtual editor click diagnostics");
        let space_width = self
            .harness
            .ctx
            .fonts_mut(|fonts| fonts.glyph_width(&egui::FontId::monospace(14.0), ' '));
        VirtualGeometry {
            viewport: event.viewport_rect,
            full_rect: event.response_rect,
            // text_left = gutter edge + 8; the clip rect starts at gutter edge + 1.
            text_left: event.text_clip_rect.left() + 7.0 - editor.scroll_x,
            scroll_y: editor.scroll_y,
            row_height: event.row_height,
            char_width: space_width,
        }
    }

    /// Screen position of `(line, column)` in the focused virtual editor.
    pub fn virtual_pos(&mut self, line: u32, column: u32) -> egui::Pos2 {
        let g = self.virtual_geometry();
        egui::pos2(
            g.text_left + (column as f32 + 0.3) * g.char_width,
            g.viewport.top() + line as f32 * g.row_height - g.scroll_y + g.row_height * 0.5,
        )
    }

    /// Clicks the focused virtual editor so the cursor lands at `(line, column)`.
    pub fn click_virtual(&mut self, line: u32, column: u32) {
        self.steps(25);
        let pos = self.virtual_pos(line, column);
        self.click_at(pos);
    }

    /// Clicks the `index`-th text field, selects its content and replaces it with `text`.
    pub fn fill_text_field(&mut self, index: usize, text: &str) {
        let rects = self.text_field_rects();
        let rect = *rects
            .get(index)
            .unwrap_or_else(|| panic!("no text field #{index}; found {}", rects.len()));
        self.click_at(rect.center());
        self.ctrl(egui::Key::A);
        if text.is_empty() {
            self.key(egui::Key::Backspace);
        } else {
            self.type_text(text);
        }
    }

    pub fn label_containing_rect(&self, text: &str) -> egui::Rect {
        self.harness.get_by_label_contains(text).rect()
    }

    pub fn click_at(&mut self, pos: egui::Pos2) {
        self.click_at_with(pos, egui::Modifiers::NONE);
    }

    pub fn click_at_with(&mut self, pos: egui::Pos2, modifiers: egui::Modifiers) {
        self.set_modifiers(modifiers);
        self.pointer_button(pos, true, modifiers);
        self.pointer_button(pos, false, modifiers);
        self.set_modifiers(egui::Modifiers::NONE);
        // A click often opens a window; egui lays new windows out invisibly first.
        self.steps(2);
    }

    pub fn double_click_at(&mut self, pos: egui::Pos2) {
        self.pointer_button(pos, true, egui::Modifiers::NONE);
        self.pointer_button(pos, false, egui::Modifiers::NONE);
        self.pointer_button(pos, true, egui::Modifiers::NONE);
        self.pointer_button(pos, false, egui::Modifiers::NONE);
        self.step();
    }

    pub fn secondary_click_at(&mut self, pos: egui::Pos2) {
        self.send(egui::Event::PointerMoved(pos));
        for pressed in [true, false] {
            self.send(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        self.step();
    }

    pub fn pointer_button(&mut self, pos: egui::Pos2, pressed: bool, modifiers: egui::Modifiers) {
        self.send(egui::Event::PointerMoved(pos));
        self.send(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        });
    }

    pub fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
        self.pointer_button(from, true, egui::Modifiers::NONE);
        let steps = 6;
        for index in 1..=steps {
            let t = index as f32 / steps as f32;
            self.send(egui::Event::PointerMoved(from.lerp(to, t)));
        }
        self.pointer_button(to, false, egui::Modifiers::NONE);
    }

    pub fn move_pointer(&mut self, pos: egui::Pos2) {
        self.send(egui::Event::PointerMoved(pos));
    }

    pub fn scroll(&mut self, pos: egui::Pos2, delta: egui::Vec2) {
        self.send(egui::Event::PointerMoved(pos));
        self.send(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta,
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        });
        self.step();
    }

    pub fn key(&mut self, key: egui::Key) {
        self.key_with(egui::Modifiers::NONE, key);
    }

    pub fn key_with(&mut self, modifiers: egui::Modifiers, key: egui::Key) {
        self.set_modifiers(modifiers);
        for pressed in [true, false] {
            self.send(egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            });
        }
        self.set_modifiers(egui::Modifiers::NONE);
        self.step();
    }

    pub fn ctrl(&mut self, key: egui::Key) {
        self.key_with(CTRL, key);
    }

    pub fn ctrl_shift(&mut self, key: egui::Key) {
        self.key_with(CTRL | egui::Modifiers::SHIFT, key);
    }

    pub fn alt(&mut self, key: egui::Key) {
        self.key_with(egui::Modifiers::ALT, key);
    }

    pub fn shift(&mut self, key: egui::Key) {
        self.key_with(egui::Modifiers::SHIFT, key);
    }

    /// Presses `keys` one after another while Shift is held down the whole time, as a user
    /// extending a selection does (Shift goes down once, so it is not a Shift double-tap).
    pub fn shift_held(&mut self, keys: &[egui::Key]) {
        // Keep consecutive calls out of the Shift double-tap window (DISCREPANCY D03).
        self.steps(30);
        self.set_modifiers(egui::Modifiers::SHIFT);
        self.step();
        for key in keys {
            for pressed in [true, false] {
                self.send(egui::Event::Key {
                    key: *key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::SHIFT,
                });
            }
        }
        self.set_modifiers(egui::Modifiers::NONE);
        self.step();
    }

    /// Sends text input to whatever widget currently has keyboard focus.
    pub fn type_text(&mut self, text: &str) {
        self.send(egui::Event::Text(text.to_owned()));
        self.step();
    }

    pub fn paste_event(&mut self, text: &str) {
        self.send(egui::Event::Paste(text.to_owned()));
        self.step();
    }

    pub fn copy_event(&mut self) {
        self.send(egui::Event::Copy);
        self.step();
    }

    pub fn cut_event(&mut self) {
        self.send(egui::Event::Cut);
        self.step();
    }

    pub fn open_file(&mut self, relative_path: &str) {
        let path = self.path(relative_path);
        self.app_mut().open_file_in_focused_pane(path);
        self.steps(2);
    }

    pub fn set_modifiers(&mut self, modifiers: egui::Modifiers) {
        self.harness.input_mut().modifiers = modifiers;
    }

    /// Taps Shift twice quickly (the "Shift x2" Search Everywhere gesture).
    pub fn press_shift_twice(&mut self) {
        for _ in 0..2 {
            self.set_modifiers(egui::Modifiers::SHIFT);
            self.step();
            self.set_modifiers(egui::Modifiers::NONE);
            self.step();
        }
    }

    pub fn git(&self, args: &[&str]) -> String {
        run_git(&self.root, args)
    }

    pub fn write(&self, relative_path: &str, content: &str) {
        let path = self.path(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    pub fn read(&self, relative_path: &str) -> String {
        fs::read_to_string(self.path(relative_path)).unwrap()
    }

    pub fn saved_state(&self) -> String {
        fs::read_to_string(&self.platform.state_path).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct VirtualGeometry {
    /// Text area (scrollbar gutters excluded).
    pub viewport: egui::Rect,
    /// Whole editor area including the scrollbars.
    pub full_rect: egui::Rect,
    pub text_left: f32,
    pub scroll_y: f32,
    pub row_height: f32,
    pub char_width: f32,
}

pub(crate) fn location(path: &Path, line: u32, character: u32) -> Location {
    let position = Position::new(line, character);
    Location::new(path.to_path_buf(), Range::new(position, position))
}

pub(crate) fn server_event(kind: LspServerEventKind, method: &str) -> LspClientEvent {
    LspClientEvent::ServerEvent(LspServerEvent {
        kind,
        method: method.to_owned(),
        message: None,
        diagnostics_count: None,
    })
}

pub(crate) fn run_git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            std::env::temp_dir().join("code-reader-test-gitconfig"),
        )
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

pub(crate) fn init_git_repository(root: &Path) {
    run_git(root, &["init", "-q", "-b", "main"]);
    run_git(root, &["config", "user.name", "Test User"]);
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "commit.gpgsign", "false"]);
    run_git(root, &["config", "core.autocrlf", "false"]);
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "--allow-empty", "-m", "initial"]);
}

fn json_escape_path(path: &Path) -> String {
    let json = serde_json::to_string(&path.display().to_string()).unwrap();
    json[1..json.len() - 1].to_owned()
}
