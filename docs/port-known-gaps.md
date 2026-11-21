# Known gaps of the egui-to-craft port

Status: 10 app tests remain ignored after the port's test phase (2026-11-20). Remove the `#[ignore]` when its gap is
fixed. Framework items (F-n) are in the port workspace's follow-up list.

| Test | How it fails | Likely cause |
|---|---|---|
| `comments::a_double_click_on_a_replace_text_opens_its_one_window` | the group's window doesn't open on the double-click | the second press goes to the canvas tool, not the comment part |
| `comments::closing_a_pop_up_saves_its_typed_text` | typing and closing aren't one undo step | the pop-up's close commits the text as a separate edit |
| `content::added_picture_resizes_repeatedly_and_drags_back_from_out_of_bounds_without_smear` | edge texels smear across the out-of-bounds half | texture sampling/clamping of the cached picture while dragged out of the page |
| `editing::image_pixels_follow_the_frame_while_a_corner_is_dragged` | the picture lags its frame by a step during a corner drag | F-126: frame and pixels read different passes |
| `editing::paragraph_edges_and_frames_are_caught_without_aiming_for_the_hairline` | a click off the text doesn't apply the typing | F-115 follow-up: the edge band and the click-off rule overlap |
| `marks::the_preview_shows_the_page_the_range_selects_under_the_marks` | the stamp preview doesn't show the selected page | F-161: the preview overlay's clip is inverted and its geometry a pass behind |
| `panel_snapshots::every_panel_matches_its_snapshot` | all 11 accessibility-tree snapshots differ after updating the dock queries | L-298 defers pixel snapshots; these text snapshots need separate review |
| `prepare::escape_in_a_name_field_leaves_the_field_not_the_table` | Escape in a field name box closes the table window | Escape reaches the window before the field gives it up |
| `split_view_edits::typing_in_one_pane_survives_an_edit_in_the_other` | the click's own edit is lost | per-pane edit state is reset by the other pane's edit |
| `split_view_render::a_late_edit_turns_the_page_in_both_panes` | the palette's page turn doesn't reach the second pane | the late edit's go-to applies to the active pane only |
