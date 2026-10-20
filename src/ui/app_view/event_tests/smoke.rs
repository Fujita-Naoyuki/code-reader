use super::support::TestApp;

#[test]
fn clicking_a_file_in_the_tree_opens_it() {
    let mut t = TestApp::plain();
    assert!(t.has_label("notes.txt"));
    t.click("notes.txt");
    assert_eq!(
        t.app().left.path.as_deref(),
        Some(t.path("notes.txt").as_path())
    );
    assert!(t.app().left.text.as_str().starts_with("alpha"));
}
