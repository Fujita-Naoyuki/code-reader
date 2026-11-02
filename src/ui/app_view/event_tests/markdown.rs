//! Markdown preview: Preview / Markdown / Both modes, pages and outline, links, code and
//! Mermaid blocks, footnotes, tables, images and rendered-text search.

use eframe::egui;

use crate::core::location::Position;

use super::super::{MarkdownFocusTarget, MarkdownMode};
use super::support::TestApp;

const DOC: &str = r#"Intro with **bold**, *em*, ~~gone~~, `tick` and $x^2$ and a [guide link](guide.md#target-section) plus [web](https://example.com/page), [far ref][far], [txt](notes.txt), [nowhere](missing.md), [bad frag](guide.md#nope), [empty]() and a far note[^far].<br>
line after break

# First Page

Plain first page paragraph.

### Section Alpha

> quoted text

- bullet item
- [x] done task
3. third item
4. fourth item

---

```rust
fn sample() {}
```

```mermaid
graph TD
  A --> B
```

```mermaid
this is not mermaid
```

| left | right |
|:-----|------:|
| 1 | **2** |

![pixel](pixel.png)
![gone](gone.png)
![remote](https://example.com/x.png)

<div>raw html</div>

### Section Beta

Beta text with [same page](#section-alpha).

## Second Page

Second page body mentions needle, a [near ref][near] and a near note[^near].

[near]: https://example.com/near

[^near]: The near footnote.

## Empty Page

## Notes

[^far]: The far footnote.

[far]: https://example.com/far
"#;

const GUIDE: &str = "# Guide\n\nGuide intro.\n\n## Target Section\n\nTarget text.\n";

fn png_bytes() -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

fn md_app() -> TestApp {
    let mut t = TestApp::builder()
        .file("doc.md", DOC)
        .file("guide.md", GUIDE)
        .file("notes.txt", "plain\n")
        .bytes("pixel.png", &png_bytes())
        .size(1600.0, 1400.0)
        .build();
    t.click_first("doc.md");
    t.steps(3);
    t
}

#[test]
fn preview_renders_the_first_page_with_navigation() {
    let mut t = md_app();
    assert_eq!(t.app().left.markdown.mode, MarkdownMode::Preview);
    assert!(t.has_label("MARKDOWN"));
    assert!(t.has_label("Pages"));
    assert!(t.has_label("1/5"));
    assert!(t.has_label("Introduction"));
    // Definitions at the end of the file still resolve on the first page.
    assert!(t.has_label("far ref"));
    assert!(!t.has_label_containing("[far ref][far]"));
    assert!(!t.has_label_containing("note[^far]"));
    assert!(t.has_label("[2]"));
    assert!(t.has_label("Footnotes"));
    assert!(t.has_label("The far footnote."));
    t.click_text_label("far ref");
    t.assert_status("Opened external URL: https://example.com/far");

    t.click_nth("Next", 0);
    assert!(t.has_label("2/5"));
    assert!(t.has_label("First Page"));
    assert!(t.has_label("Section Alpha"));
    assert!(t.has_label("quoted text"));
    assert!(t.has_label("bullet item"));
    assert!(t.has_label("3."));
    assert!(t.has_label("rust"));
    assert!(t.has_label("mermaid"));
    assert!(t.has_label_containing("Mermaid rendering failed"));
    assert!(t.has_label("left"));
    assert!(t.has_label_containing("[image: gone] not found"));
    assert!(t.has_label_containing("[image: remote] https://example.com/x.png"));
    assert!(t.has_label("<div>raw html</div>"));

    t.click_nth("Previous", 0);
    assert!(t.has_label("1/5"));

    t.ctrl(egui::Key::PageDown);
    t.ctrl(egui::Key::PageDown);
    assert!(t.has_label("3/5"));
    assert!(t.has_label("Second Page"));
    assert!(t.has_label("Footnotes"));
    t.ctrl(egui::Key::PageDown);
    assert!(t.has_label("4/5"));
    assert!(t.has_label("No rendered text for this page."));
    t.ctrl(egui::Key::PageDown);
    assert!(t.has_label("5/5"));
    assert!(t.has_label("The far footnote."));
    t.ctrl(egui::Key::PageDown);
    assert!(t.has_label("5/5"), "stays on the last page");
    t.ctrl(egui::Key::PageUp);
    assert!(t.has_label("4/5"));
}

#[test]
fn pages_list_rows_select_pages() {
    let mut t = md_app();
    // Page rows are painted; they sit below the "Pages" pane label (the header toggle with
    // the same text comes first), one text row each.
    let pages = t.nth_label_rect("Pages", 1);
    let row_height = 18.0;
    t.click_at(egui::pos2(
        pages.left() + 30.0,
        pages.bottom() + 6.0 + row_height * 2.5,
    ));
    assert!(t.has_label("3/5"));
    assert_eq!(
        t.app().left.markdown.focus_target,
        MarkdownFocusTarget::Body
    );
}

#[test]
fn mode_toggles_and_alt_shortcuts() {
    let mut t = md_app();
    t.click_text_label("Markdown");
    assert_eq!(t.app().left.markdown.mode, MarkdownMode::Markdown);
    assert!(
        !t.has_label("Outline"),
        "side panes are hidden in source mode"
    );
    t.click_text_label("Both");
    assert_eq!(t.app().left.markdown.mode, MarkdownMode::Both);
    t.click_text_label("Preview");
    assert_eq!(t.app().left.markdown.mode, MarkdownMode::Preview);

    t.alt(egui::Key::Num2);
    assert_eq!(t.app().left.markdown.mode, MarkdownMode::Markdown);
    t.alt(egui::Key::Num3);
    assert_eq!(t.app().left.markdown.mode, MarkdownMode::Both);
    t.alt(egui::Key::Num1);
    assert_eq!(t.app().left.markdown.mode, MarkdownMode::Preview);
    t.alt(egui::Key::Num1);
    assert_eq!(t.app().left.markdown.mode, MarkdownMode::Preview);
}

#[test]
fn outline_and_pages_panes_toggle() {
    let mut t = md_app();
    t.click_nth("Next", 0);
    t.click_text_label("Outline");
    assert!(t.app().left.markdown.show_outline);
    assert!(t.has_label("Section Beta"));
    t.click_nth("Section Beta", 0);
    assert_eq!(
        t.app().left.markdown.active_heading_id.as_deref(),
        Some("h3-section-beta")
    );

    t.click_text_label("Pages");
    assert!(!t.app().left.markdown.show_pages);
    t.click_text_label("Pages");
    assert!(t.app().left.markdown.show_pages);

    // Both mode shows the outline next to the source editor.
    t.alt(egui::Key::Num3);
    assert!(t.has_label("Section Beta"));
    // The outline of a page without H3+ headings is empty.
    t.alt(egui::Key::Num1);
    t.click_nth("Previous", 0);
    assert!(t.has_label("No headings"));
    t.click_text_label("Outline");
    assert!(!t.app().left.markdown.show_outline);
}

#[test]
fn pages_and_outline_take_the_keyboard_when_clicked_or_with_f6() {
    let mut t = md_app();
    t.click_text_label("Outline");
    // The pane titles are the second "Pages" / "Outline" (after the header toggles).
    t.click_nth("Pages", 1);
    assert_eq!(
        t.app().left.markdown.focus_target,
        MarkdownFocusTarget::Pages
    );
    assert!(t.app().context_shortcuts()[0].keys == "Up/Down");
    t.key(egui::Key::ArrowDown);
    assert!(t.has_label("2/5"));
    t.key(egui::Key::ArrowUp);
    t.key(egui::Key::Enter);
    assert_eq!(
        t.app().left.markdown.focus_target,
        MarkdownFocusTarget::Body
    );

    t.click_nth("Next", 0);
    t.click_nth("Outline", 1);
    assert_eq!(
        t.app().left.markdown.focus_target,
        MarkdownFocusTarget::Outline
    );
    t.key(egui::Key::ArrowDown);
    assert_eq!(
        t.app().left.markdown.active_heading_id.as_deref(),
        Some("h3-section-beta")
    );
    t.key(egui::Key::ArrowUp);
    assert_eq!(
        t.app().left.markdown.active_heading_id.as_deref(),
        Some("h3-section-alpha")
    );
    t.key(egui::Key::Enter);
    assert_eq!(
        t.app().left.markdown.focus_target,
        MarkdownFocusTarget::Body
    );

    // F6 / Shift+F6 cycle Pages -> Body -> Outline.
    t.key(egui::Key::F6);
    assert_eq!(
        t.app().left.markdown.focus_target,
        MarkdownFocusTarget::Outline
    );
    t.key(egui::Key::F6);
    assert_eq!(
        t.app().left.markdown.focus_target,
        MarkdownFocusTarget::Pages
    );
    t.shift(egui::Key::F6);
    assert_eq!(
        t.app().left.markdown.focus_target,
        MarkdownFocusTarget::Outline
    );

    // Hiding a pane while it has the keyboard moves it back to the body.
    t.click_text_label("Outline");
    assert_eq!(
        t.app().left.markdown.focus_target,
        MarkdownFocusTarget::Body
    );
    t.key(egui::Key::F6);
    t.alt(egui::Key::Num3);
    assert_eq!(
        t.app().left.markdown.focus_target,
        MarkdownFocusTarget::Body
    );
}

#[test]
fn local_markdown_links_open_with_fragments() {
    let mut t = md_app();
    t.click_text_label("guide link");
    t.assert_status_containing("Opened Markdown link:");
    assert_eq!(t.app().left.path(), Some(t.path("guide.md").as_path()));
    assert_eq!(
        t.app().left.markdown.active_page_id.as_deref(),
        Some("target-section")
    );
    assert_eq!(t.app().left.cursor_position, Some(Position::new(4, 0)));

    t.alt(egui::Key::ArrowLeft);
    assert_eq!(t.app().left.path(), Some(t.path("doc.md").as_path()));

    t.click_text_label("bad frag");
    t.assert_status("Fragment not found: #nope");

    t.alt(egui::Key::ArrowLeft);
    t.click_nth("Next", 0);
    t.click_text_label("same page");
    assert_eq!(
        t.app().left.markdown.active_heading_id.as_deref(),
        Some("h3-section-alpha")
    );
}

#[test]
fn other_links_report_or_open_externally() {
    let mut t = md_app();
    t.click_text_label("web");
    assert_eq!(
        t.platform.opened_urls.borrow().as_slice(),
        ["https://example.com/page".to_owned()]
    );
    t.assert_status("Opened external URL: https://example.com/page");

    t.platform.open_url_fails.set(true);
    t.click_text_label("web");
    t.assert_status("Failed to open external URL: https://example.com/page (no browser)");

    t.ctrl(egui::Key::PageDown);
    t.ctrl(egui::Key::PageDown);
    t.click_text_label("near ref");
    t.assert_status("Failed to open external URL: https://example.com/near (no browser)");
    t.ctrl(egui::Key::PageUp);
    t.ctrl(egui::Key::PageUp);
    t.click_text_label("txt");
    t.assert_status_containing("Only local Markdown links are supported:");
    t.click_text_label("nowhere");
    t.assert_status_containing("Markdown link target not found:");
    t.click_text_label("empty");
    t.assert_status("Markdown link target is empty.");
}

#[test]
fn code_and_mermaid_copy_buttons() {
    let mut t = md_app();
    t.click_nth("Next", 0);
    // Copy buttons in order: rust block, mermaid diagram, broken mermaid (as code).
    t.click_nth("Copy", 0);
    assert_eq!(
        t.platform.clipboard_text().as_deref(),
        Some("fn sample() {}")
    );
    t.click_nth("Copy", 1);
    assert_eq!(
        t.platform.clipboard_text().as_deref(),
        Some("graph TD\n  A --> B")
    );
    t.click_nth("Copy", 2);
    assert_eq!(
        t.platform.clipboard_text().as_deref(),
        Some("this is not mermaid")
    );
}

#[test]
fn footnote_reference_and_back_button() {
    let mut t = md_app();
    t.ctrl(egui::Key::PageDown);
    t.ctrl(egui::Key::PageDown);
    assert!(t.has_label("The near footnote."));
    // Footnotes are numbered in definition order: the near one comes first.
    t.click_text_label("[1]");
    t.steps(2);
    assert!(t.app().left.markdown.pending_footnote_scroll.is_none());
    t.click_text_label("Back");
    t.steps(2);
    assert!(t.app().left.markdown.pending_footnote_scroll.is_none());
}

#[test]
fn rendered_search_finds_text_across_pages() {
    let mut t = md_app();
    t.ctrl(egui::Key::F);
    assert!(t.app().left.markdown.search.open);
    assert!(!t.app().left.source_search.open);
    t.type_text("needle");
    assert!(t.has_label("1/1"));
    t.steps(3);
    assert!(t.has_label("3/5"), "jumps to the page with the match");

    t.fill_markdown_search("page");
    let count = t.app().left.markdown.search.match_count;
    assert!(count >= 3);
    t.key(egui::Key::Enter);
    assert_eq!(t.app().left.markdown.search.current_match, 1);
    t.steps(30);
    t.shift(egui::Key::Enter);
    assert_eq!(t.app().left.markdown.search.current_match, 0);
    t.click_nth("Next", 0);
    t.click_nth("Previous", 0);
    t.key(egui::Key::F3);
    t.steps(30);
    t.shift(egui::Key::F3);
    t.fill_markdown_search("zzzz");
    assert!(t.has_label("No results"));
    t.key(egui::Key::F3);
    t.key(egui::Key::Escape);
    assert!(!t.app().left.markdown.search.open);
    t.ctrl(egui::Key::F);
    t.click("Close");
    assert!(!t.app().left.markdown.search.open);
}

#[test]
fn markdown_source_mode_edits_and_tracks_the_active_page() {
    let mut t = md_app();
    t.alt(egui::Key::Num2);
    assert_eq!(t.app().left.cursor_position, Some(Position::new(0, 0)));
    t.ctrl(egui::Key::F);
    assert!(
        t.app().left.source_search.open,
        "source mode searches the source"
    );
    t.type_text("Second Page");
    t.steps(3);
    assert_eq!(
        t.app().left.markdown.active_page_id.as_deref(),
        Some("second-page")
    );
    t.click("Close");
    t.click_text(0, 0);
    t.type_text("X");
    assert!(t.app().left.text.as_str().starts_with("XIntro"));
    t.ctrl(egui::Key::S);
    t.assert_status_containing("Saved file:");

    // Both mode: page navigation moves the source cursor to the page start.
    t.alt(egui::Key::Num3);
    t.click_nth("Next", 0);
    let first_page_line = DOC.lines().position(|line| line == "# First Page").unwrap();
    assert_eq!(
        t.app().left.cursor_position,
        Some(Position::new(first_page_line as u32, 0))
    );
    t.ctrl(egui::Key::PageDown);
    t.click_nth("Previous", 0);
}

#[test]
fn markdown_mode_is_remembered_per_file() {
    let mut t = md_app();
    t.alt(egui::Key::Num2);
    t.click_first("guide.md");
    assert_eq!(t.app().left.markdown.mode, MarkdownMode::Preview);
    t.click_first("doc.md");
    assert_eq!(t.app().left.markdown.mode, MarkdownMode::Markdown);
}

#[test]
fn pages_pane_can_be_resized_by_dragging() {
    let mut t = md_app();
    let before = t.app().left.markdown.pages_width;
    let pages = t.nth_label_rect("Pages", 1);
    let handle = egui::pos2(pages.left() + before + 4.0, pages.bottom() + 100.0);
    t.drag(handle, handle + egui::vec2(80.0, 0.0));
    assert!(t.app().left.markdown.pages_width > before + 40.0);

    t.alt(egui::Key::Num3);
    let width = t.app().left.markdown.pages_width;
    let pages = t.nth_label_rect("Pages", 1);
    let handle = egui::pos2(pages.left() + width + 4.0, pages.bottom() + 100.0);
    t.drag(handle, handle - egui::vec2(60.0, 0.0));
    assert!(t.app().left.markdown.pages_width < width);
}

#[test]
fn empty_and_heading_only_documents() {
    let mut t = TestApp::builder()
        .file("empty.md", "")
        .file("only.md", "# Only\n")
        .file("deep.md", "Intro\n\n### Deep heading\n\nbody\n")
        .build();
    t.click_first("empty.md");
    t.steps(2);
    assert!(t.has_label("Empty Markdown document"));
    assert!(t.has_label("No pages"));
    t.click_text_label("Outline");
    assert!(t.has_label("No outline"));
    t.ctrl(egui::Key::PageDown);

    t.click_first("only.md");
    t.steps(2);
    assert!(t.has_label("No rendered text for this page."));

    t.click_first("deep.md");
    t.steps(2);
    assert!(t.has_label("Deep heading"));
}

#[test]
fn new_markdown_files_get_a_title_heading() {
    let mut t = TestApp::builder().file("a.txt", "").build();
    t.click_first("New File");
    t.type_text("Notes.md");
    t.key(egui::Key::Enter);
    assert_eq!(t.read("Notes.md"), "# Notes\n");
    assert!(t.has_label("Notes"));
}
