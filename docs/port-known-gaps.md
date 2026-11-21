# Known gaps of the egui-to-craft port

Status: 10 app tests remain ignored after the port's test phase (2026-11-20). Remove the `#[ignore]` when its gap is
fixed. Framework items (F-n) are in the port workspace's follow-up list.

| Test | How it fails | Likely cause |
|---|---|---|
| `comments::a_double_click_on_a_replace_text_opens_its_one_window` | the group's window doesn't open on the double-click | the comment part receives the first press and release; the second press reaches the comment canvas tool with click count two |
| `editing::paragraph_edges_and_frames_are_caught_without_aiming_for_the_hairline` | a click off the text doesn't apply the typing | F-115 follow-up: the edge band and the click-off rule overlap |
| `marks::the_preview_shows_the_page_the_range_selects_under_the_marks` | the stamp preview doesn't show the selected page | F-161: the preview overlay's clip is inverted and its geometry a pass behind |
| `panel_snapshots::every_panel_matches_its_snapshot` | all 11 accessibility-tree snapshots differ after updating the dock queries | L-298 defers pixel snapshots; these text snapshots need separate review |
| `split_view_render::a_late_edit_turns_the_page_in_both_panes` | the palette does not turn the page | the port's palette control ignores the search value; production puts it in the query, which the shared palette now keeps in its own session. Typing the query and pressing Enter or activating the matching row also left rotation at zero; the pane rendering assertion has not yet been reached |
