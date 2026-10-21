//! Markdown preview rendering details: inline formatting outside links, tables, heading
//! levels, fragment and path resolution in links, the light theme, and Markdown-source
//! interactions (scrolling, bookmarks, search while typing).

use eframe::egui;

use super::super::MarkdownMode;
use super::support::TestApp;

const INLINE_DOC: &str = r#"# Inline Page

Plain **strong** *emphasis* ~~deleted~~ and `code` with a hard break\
on the next line.

> A quoted note.

| Left | Center | Right |
|:-----|:------:|------:|
| [cell link](https://example.com/c) | ![cell image](pic.png) | [ref cell][refc] |
| `code cell` | *em cell* | ![cell ref image][imgref] |

[refc]: https://example.com/refc

Widgets paragraph with a [link here](#h4-level-four) then a hard break\
and ![inline ref][imgref] plus [](https://example.com/empty-label) and searchable needle.

[imgref]: pic.png

### Level Three

#### Level Four

##### Level Five

###### Level Six

Text under level six with needle too.

## A Very Long Page Title That Will Not Fit Into The Narrow Pages Pane At All

Body of the long page.

## Fragments

[exact slug](#h3-level-three) [trailing dash](#level-three-) [punctuation only](#!!!)
[percent upper](%46ragments.md) [percent lower](%66ragments.md) [percent bad](%zzfile.md)
"#;

fn png_bytes() -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 255, 255]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

fn inline_app(light: bool) -> TestApp {
    let mut builder = TestApp::builder()
        .file("inline.md", INLINE_DOC)
        .bytes("pic.png", &png_bytes())
        .size(1600.0, 1400.0);
    if light {
        builder = builder.light();
    }
    let mut t = builder.build();
    t.click_first("inline.md");
    t.steps(3);
    t
}

#[test]
fn inline_formatting_tables_and_heading_levels_render() {
    let mut t = inline_app(false);
    assert!(t.has_label_containing("Plain strong emphasis deleted and code"));
    assert!(t.has_label_containing("cell link (https://example.com/c)"));
    assert!(t.has_label_containing("[image: cell image]"));
    assert!(t.has_label("Level Four"));
    assert!(t.has_label("Level Five"));
    assert!(t.has_label("Level Six"));
    // The page title is longer than the pages pane and gets an ellipsis.
    assert!(t.app().left.markdown_show_pages);

    // Clicking a heading in the body selects it; clicking the body focuses it.
    t.click_nth("Level Four", 0);
    assert_eq!(
        t.app().left.markdown_active_heading_id.as_deref(),
        Some("h4-level-four")
    );
    let body = t.label_rect("Level Six");
    t.click_at(egui::pos2(body.right() + 200.0, body.bottom() + 120.0));

    // Links without text show their URL.
    t.click_text_label("https://example.com/empty-label");
    assert_eq!(
        t.platform.opened_urls.borrow().last().map(String::as_str),
        Some("https://example.com/empty-label")
    );
}

#[test]
fn light_theme_renders_with_light_colors() {
    let mut t = inline_app(true);
    assert!(!t.harness.ctx.global_style().visuals.dark_mode);
    t.ctrl(egui::Key::F);
    t.type_text("needle");
    t.steps(3);
    t.alt(egui::Key::Num2);
    t.steps(2);
    t.alt(egui::Key::Num3);
    t.steps(2);

    let mut t = TestApp::builder()
        .file("data.csv", "a,b\n1,2\n")
        .file(
            "src/main.rs",
            "struct Point { x: u32 }\nfn main() {\n    let p = Point { x: 1 };\n}\n",
        )
        .light()
        .build();
    t.click_first("data.csv");
    t.steps(2);
    t.click("src");
    t.click_first("main.rs");
    t.click_text(2, 13);
    t.wait_until("highlight", |app| app.left.syntax_highlight_cache.is_some());
    t.steps(2);
    t.ctrl(egui::Key::F);
    t.type_text("Point");
    t.steps(2);
}

#[test]
fn search_scrolls_to_headings_and_link_paragraphs() {
    let mut t = inline_app(false);
    t.ctrl(egui::Key::F);
    t.type_text("needle");
    assert!(t.app().left.markdown_search.match_count >= 2);
    t.steps(3);
    t.key(egui::Key::Enter);
    t.steps(3);
    t.fill_markdown_search("Level Five");
    t.steps(3);
    assert!(t.app().left.markdown_search.match_count >= 1);
}

#[test]
fn fragment_links_resolve_exact_slugs_and_trailing_dashes() {
    let mut t = inline_app(false);
    t.ctrl(egui::Key::PageDown);
    t.ctrl(egui::Key::PageDown);
    assert!(t.has_label("Fragments"));
    t.click_text_label("exact slug");
    assert_eq!(
        t.app().left.markdown_active_heading_id.as_deref(),
        Some("h3-level-three")
    );
    t.alt(egui::Key::ArrowLeft);
    t.ctrl(egui::Key::PageDown);
    t.ctrl(egui::Key::PageDown);
    t.click_text_label("trailing dash");
    assert_eq!(
        t.app().left.markdown_active_heading_id.as_deref(),
        Some("h3-level-three")
    );
    t.alt(egui::Key::ArrowLeft);
    t.ctrl(egui::Key::PageDown);
    t.ctrl(egui::Key::PageDown);
    t.click_text_label("punctuation only");
    t.assert_status("Fragment not found: #!!!");
}

#[test]
fn percent_encoded_and_absolute_link_targets() {
    let mut t = inline_app(false);
    std::fs::write(t.path("Fragments.md"), "# Fragments target\n").unwrap();
    std::fs::write(t.path("fragments.md"), "# fragments target\n").ok();
    t.ctrl(egui::Key::PageDown);
    t.ctrl(egui::Key::PageDown);
    t.click_text_label("percent upper");
    t.assert_status_containing("Opened Markdown link:");
    t.alt(egui::Key::ArrowLeft);
    t.ctrl(egui::Key::PageDown);
    t.ctrl(egui::Key::PageDown);
    t.click_text_label("percent lower");
    t.alt(egui::Key::ArrowLeft);
    t.ctrl(egui::Key::PageDown);
    t.ctrl(egui::Key::PageDown);
    t.click_text_label("percent bad");
    t.assert_status_containing("Markdown link target not found:");

    // Absolute paths for links and images.
    let absolute_md = t.path("abs.md");
    let picture = t.path("pic.png");
    std::fs::write(
        &absolute_md,
        format!(
            "# Abs\n\n[back home]({})\n\n![absolute picture]({})\n",
            t.path("inline.md").display().to_string().replace('\\', "/"),
            picture.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    t.app_mut().next_workspace_change_poll = std::time::Instant::now();
    t.wait_until("abs.md discovered", |app| {
        app.file_tree
            .files
            .iter()
            .any(|path| path.ends_with("abs.md"))
    });
    t.click_first("abs.md");
    t.steps(3);
    t.click_text_label("back home");
    assert_eq!(
        t.app().left.path.as_deref(),
        Some(t.path("inline.md").as_path())
    );
}

#[test]
fn markdown_source_scrolling_bookmarks_and_live_search() {
    let long = (1..=200)
        .map(|n| format!("Line {n} of markdown text\n"))
        .collect::<String>();
    let mut t = TestApp::builder()
        .file("long.md", &format!("# Long\n\n{long}"))
        .build();
    t.click_first("long.md");
    t.alt(egui::Key::Num2);

    // Bookmark gutter in source mode.
    t.steps(25);
    let star = t.gutter_star_pos(0, 203);
    t.click_at(star);
    t.assert_status_containing("bookmark: long.md:");

    t.key(egui::Key::PageDown);
    assert!(t.app().left.scroll_y > 0.0);
    t.key(egui::Key::PageDown);
    t.key(egui::Key::PageUp);

    // Typing while the source search is open keeps searching.
    t.ctrl(egui::Key::F);
    t.type_text("Line 1");
    t.click_text(0, 0);
    t.type_text("X");
    assert!(t.app().left.text.as_str().starts_with("X# Long"));
}

#[test]
fn empty_markdown_files_switch_modes_without_pages() {
    let mut t = TestApp::builder().file("empty.md", "").build();
    t.click_first("empty.md");
    t.alt(egui::Key::Num2);
    assert_eq!(t.app().left.markdown_mode, MarkdownMode::Markdown);
    t.alt(egui::Key::Num3);
    t.steps(2);
}

#[test]
fn page_rows_in_both_mode_move_the_source_cursor() {
    let mut t = inline_app(false);
    t.alt(egui::Key::Num3);
    let pages = t.nth_label_rect("Pages", 1);
    t.click_at(egui::pos2(
        pages.left() + 30.0,
        pages.bottom() + 6.0 + 18.0 * 1.5,
    ));
    assert_eq!(t.app().left.markdown_mode, MarkdownMode::Both);
    assert!(t.app().left.cursor_position.unwrap().line > 0);
}

#[test]
fn search_box_takes_enter_in_the_same_frame_it_gains_focus() {
    let mut t = inline_app(false);
    t.ctrl(egui::Key::F);
    t.type_text("needle");
    t.click_at(egui::pos2(1000.0, 700.0));
    // Ctrl+F and Enter arrive together: the box regains focus and moves to the next match.
    t.set_modifiers(super::support::CTRL);
    t.harness.event(egui::Event::Key {
        key: egui::Key::F,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: super::support::CTRL,
    });
    t.set_modifiers(egui::Modifiers::NONE);
    t.harness.event(egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    t.step();
    t.steps(30);
    t.harness.event(egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::SHIFT,
    });
    t.step();
    t.steps(2);
}

#[test]
fn rendered_search_box_handles_enter_in_the_frame_it_gets_focus() {
    let mut t = inline_app(false);
    t.ctrl(egui::Key::F);
    t.type_text("needle");
    t.click_at(egui::pos2(1000.0, 700.0));
    t.keys_in_one_frame(&[
        (egui::Key::F, super::support::CTRL),
        (egui::Key::Enter, egui::Modifiers::NONE),
    ]);
    assert_eq!(t.app().left.markdown_search.current_match, 1);
    t.click_at(egui::pos2(1000.0, 700.0));
    t.steps(40);
    t.keys_in_one_frame(&[
        (egui::Key::F, super::support::CTRL),
        (egui::Key::Enter, egui::Modifiers::SHIFT),
    ]);
    // Ctrl+F keeps the current match, so Shift+Enter steps back from it.
    assert_eq!(t.app().left.markdown_search.current_match, 0);
}

#[test]
fn markdown_source_mode_tolerates_unfinished_links() {
    let mut t = TestApp::builder()
        .file(
            "draft.md",
            "# Draft\n\nAn [open bracket and a [label] without a target.\n",
        )
        .build();
    t.click_first("draft.md");
    t.alt(egui::Key::Num2);
    t.steps(2);
    assert_eq!(t.app().left.markdown_mode, MarkdownMode::Markdown);
}

#[test]
fn shortcut_reference_links_and_odd_heading_slugs() {
    let source = "# Odd Headings\n\nSee [collapsed][] and [shortcut] links.\n\n\
        [collapsed]: https://example.com/c\n[shortcut]: https://example.com/s\n\n\
        ### Ends with punctuation !\n\nx\n\n### !!!\n\ny\n\n\
        [to punctuation](#ends-with-punctuation) [to section](#section)\n";
    let mut t = TestApp::builder().file("odd.md", source).build();
    t.click_first("odd.md");
    t.steps(2);
    t.click_text_label("collapsed");
    t.assert_status("Opened external URL: https://example.com/c");
    t.click_text_label("to punctuation");
    assert_eq!(
        t.app().left.markdown_active_heading_id.as_deref(),
        Some("h3-ends-with-punctuation")
    );
    t.alt(egui::Key::ArrowLeft);
    t.click_text_label("to section");
    assert_eq!(
        t.app().left.markdown_active_heading_id.as_deref(),
        Some("h3-section")
    );
}
