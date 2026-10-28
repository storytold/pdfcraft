# PdfCraft Enhancements & Contribution Modules

This directory contains modular enhancement proposals, implementation guides, and pull request specifications designed for contributing back to the upstream [`storytold/pdfcraft`](https://github.com/storytold/pdfcraft) repository.

All improvements strictly follow the project's governance rules:
- **`AGENTS.md` §1:** Asset policy (clean licensing, zero Adobe-authored visual assets).
- **`AGENTS.md` §2:** Clean-room reverse engineering standards.
- **`AGENTS.md` §4:** Never Crash policy (infallible error paths, bounded recursion, memory budgets, zero panics).
- **`CLAUDE.md`:** Headless parity (`crates/automation`), rigorous testing, Clippy compliance.

---

## Active Contribution Modules & Implemented Fixes

| Module / PR | Focus Area | Status | Target Issues / Gaps |
|---|---|---|---|
| [`09_fix_tool_icon_sizing_and_contrast.md`](pr_proposals/09_fix_tool_icon_sizing_and_contrast.md) | `crates/ui-egui` | **Implemented** | Fixes tool icon sizing, thin stroke, and contrast |
| [`01_fix_path_segment_operators_before_m.md`](pr_proposals/01_fix_path_segment_operators_before_m.md) | `crates/edit` | **Implemented & Tested** | Fixes crash bug **#3581** (Kurbo `BezPath` panic) |
| [`02_fix_surplus_operand_tj_extraction.md`](pr_proposals/02_fix_surplus_operand_tj_extraction.md) | `crates/edit`, `crates/a11y`, `crates/redact` | **Implemented & Tested** | Fixes correctness bug **#3569** (stack order on `TJ`) |
| [`05_fix_measure_snap_trailing_comment.md`](pr_proposals/05_fix_measure_snap_trailing_comment.md) | `crates/measure` | **Implemented & Tested** | Fixes **#3570** (Measure snap unmasking commented geometry) |
| [`06_fix_paragraph_blank_line_grouping.md`](pr_proposals/06_fix_paragraph_blank_line_grouping.md) | `crates/edit` | **Implemented & Tested** | Fixes **#3577** (Blank lines & headings swallowed into paragraph) |
| [`07_fix_print_paper_size_restore_from_label.md`](pr_proposals/07_fix_print_paper_size_restore_from_label.md) | `crates/ui-egui` | **Implemented & Tested** | Fixes **#3564** (Paper size stuck on 4x6 after leaving Label mode) |
| [`08_fix_multiline_highlight_quadpoints_hover.md`](pr_proposals/08_fix_multiline_highlight_quadpoints_hover.md) | `crates/ui-egui` | **Implemented & Tested** | Fixes **#3541**, **#3620** (Highlight hover over unhighlighted text) |
| [`10_fix_marked_content_nesting_cap.md`](pr_proposals/10_fix_marked_content_nesting_cap.md) | `crates/edit` | **Implemented & Tested** | Fixes **#3573** (Marked-content nesting cap losing `/ActualText`) |
| [`11_prevent_overflow_rotating_images.md`](pr_proposals/11_prevent_overflow_rotating_images.md) | `crates/automation` | **Implemented & Tested** | Fixes **#3187** (Integer overflow when rotating images via automation) |
| [`12_control_channel_accept_backoff.md`](pr_proposals/12_control_channel_accept_backoff.md) | `crates/ui-egui` | **Implemented** | Fixes **#3631** (Control channel tight CPU spin on accept failure) |
| [`13_fix_preferences_scrollbar_overflow.md`](pr_proposals/13_fix_preferences_scrollbar_overflow.md) | `crates/ui-egui` | **Implemented** | Fixes **#3557** (Preferences scrollbar overlapping UI Theme card and OK button) |
| [`14_prevent_stack_overflow_deep_name_tree.md`](pr_proposals/14_prevent_stack_overflow_deep_name_tree.md) | `crates/organize` | **Implemented & Tested** | Fixes **#3309** (Stack overflow while deleting pages in deep name tree) |
| [`15_acroform_dr_fonts_indirect_refs.md`](pr_proposals/15_acroform_dr_fonts_indirect_refs.md) | `crates/forms` | **Implemented & Tested** | Fixes **#3441** (Adobe Reader background covering text after field edit) |
| [`16_save_commits_open_text_boxes.md`](pr_proposals/16_save_commits_open_text_boxes.md) | `crates/ui-egui` | **Implemented** | Fixes **#3546**, **#3496** (Save commits open Edit Text boxes and line editors) |
| [`17_highlight_fields_fillable_only.md`](pr_proposals/17_highlight_fields_fillable_only.md) | `crates/ui-egui` | **Implemented** | Fixes **#3584** (Highlight fields marks only fillable fields, skipping read-only) |
| [`18_center_search_matches_in_viewport.md`](pr_proposals/18_center_search_matches_in_viewport.md) | `crates/ui-egui` | **Implemented** | Fixes **#3547**, **#3530** (Find search centres active match in viewport) |
| [`19_button_caption_respects_da.md`](pr_proposals/19_button_caption_respects_da.md) | `crates/forms` | **Implemented** | Fixes **#3543** (Push button caption uses font, size and color from `/DA`) |
| [`20_page_tree_walkers_skip_repeated_children.md`](pr_proposals/20_page_tree_walkers_skip_repeated_children.md) | `crates/annot`, `crates/forms`, `crates/organize` | **Implemented** | Fixes **#3608** (Page-tree walkers skip duplicate children on queueing) |
| [`21_remember_pdf_scroll_position.md`](pr_proposals/21_remember_pdf_scroll_position.md) | `crates/ui-egui` | **Implemented & Tested** | Fixes **#3555** (Persist vertical scroll position across sessions) |
| [`22_skip_malformed_operators.md`](pr_proposals/22_skip_malformed_operators.md) | `vendor/hayro-syntax`, `crates/content` | **Implemented & Tested** | Fixes **#3568**, **#3636** (Step over malformed operators instead of ending page) |
| [`23_link_navigation_history_shortcuts.md`](pr_proposals/23_link_navigation_history_shortcuts.md) | `crates/ui-egui` | **Implemented** | Fixes **#3632** (Alt+Left/Right navigation history jump back/forward) |
| [`24_hide_annotations_with_off_oc_layers.md`](pr_proposals/24_hide_annotations_with_off_oc_layers.md) | `vendor/hayro-interpret` | **Implemented** | Fixes **#3624**, **#3572** (Hide annotations whose `/OC` is an OFF layer) |
| [`25_decode_javascript_af_hex_escapes.md`](pr_proposals/25_decode_javascript_af_hex_escapes.md) | `crates/forms` | **Implemented & Tested** | Fixes **#3623** (Decode JavaScript `\xHH` and control escapes in AF strings) |
| [`26_preserve_content_boundaries_image_merge.md`](pr_proposals/26_preserve_content_boundaries_image_merge.md) | `crates/optimize` | **Implemented & Tested** | Fixes **#3622**, **#3507** (Preserve stream comments when merging image fragments) |
| [`27_doubled_sign_numbers_content_lexer.md`](pr_proposals/27_doubled_sign_numbers_content_lexer.md) | `vendor/hayro-syntax` | **Implemented** | Fixes **#3630**, **#3565** (Doubled-sign `--280` parsed as single number, avoiding blank page) |
| [`28_unlink_unused_image_resources_on_delete.md`](pr_proposals/28_unlink_unused_image_resources_on_delete.md) | `crates/edit` | **Implemented & Tested** | Fixes **#3523**, **#3511** (Unlink unused image resources on delete/replace) |
| [`29_pages_range_input_for_extraction.md`](pr_proposals/29_pages_range_input_for_extraction.md) | `crates/ui-egui` | **Implemented** | Fixes **#3593** (Add page range input e.g. `1-5, 8, 11-13` for extraction) |
| [`30_spaces_in_non_embedded_japanese_cid_fonts.md`](pr_proposals/30_spaces_in_non_embedded_japanese_cid_fonts.md) | `vendor/hayro-interpret` | **Implemented** | Fixes **#3601** (Spaces in non-embedded Japanese CID fonts fallback to space) |
| [`31_macos_devanagari_font_fallback_paths.md`](pr_proposals/31_macos_devanagari_font_fallback_paths.md) | `crates/ui-egui` | **Implemented** | Fixes **#3598**, **#3597** (Correct macOS Devanagari font fallback candidate paths) |
| [`32_installed_hangul_korean_font_fallback.md`](pr_proposals/32_installed_hangul_korean_font_fallback.md) | `crates/ui-egui` | **Implemented** | Fixes **#3505**, **#3517** (Installed Hangul system font fallback for UI/tabs/bookmarks) |
| [`33_organized_file_and_edit_menu_groups.md`](pr_proposals/33_organized_file_and_edit_menu_groups.md) | `crates/ui-egui` | **Implemented** | Fixes **#3524** (Organize File & Edit menus with visual separators and logical grouping) |
| [`34_exclude_text_outside_form_bounds_extraction.md`](pr_proposals/34_exclude_text_outside_form_bounds_extraction.md) | `crates/render` | **Implemented** | Fixes **#3628** (Exclude text outside transformed form bounds from text extraction & search) |
| [`PR_0033_edit_skip_uninitialized_path_segments.md`](pr_proposals/PR_0033_edit_skip_uninitialized_path_segments.md) | `crates/edit` | **Implemented & Tested** | Fixes **#3581**, **#3621** (Guard image scan against uninitialized subpaths in kurbo) |
| [`PR_0034_print_promote_appearances_missing_subtype.md`](pr_proposals/PR_0034_print_promote_appearances_missing_subtype.md) | `crates/print` | **Implemented & Tested** | Fixes **#3532**, **#3633** (Promote appearances without Subtype to Form XObjects in vector print) |
| [`PR_0035_forms_preserve_comments_across_content_streams.md`](pr_proposals/PR_0035_forms_preserve_comments_across_content_streams.md) | `crates/forms` | **Implemented & Tested** | Fixes **#3502**, **#3625** (Preserve comments across content stream boundaries in form detection) |
| [`PR_0036_page_tree_walk_deduplicate_repeated_kids.md`](pr_proposals/PR_0036_page_tree_walk_deduplicate_repeated_kids.md) | `crates/annot`, `crates/forms`, `crates/organize` | **Implemented & Tested** | Fixes **#1798**, **#1926**, **#3608** (Deduplicate repeated kids and bound page-tree memory) |
| [`PR_0037_syntax_content_stream_step_over_malformed_tokens.md`](pr_proposals/PR_0037_syntax_content_stream_step_over_malformed_tokens.md) | `vendor/hayro-syntax` | **Implemented & Tested** | Fixes **#3568**, **#3636** (Step over malformed tokens and overlong operands instead of aborting page) |
| [`03_math_and_symbol_font_mapping.md`](pr_proposals/03_math_and_symbol_font_mapping.md) | `crates/fonts`, `crates/render` | **Architecture & Design** | Resolves **#3519** (math operators `+`, `-`, Greek glyphs) |
| [`04_canvas_gutter_and_quick_bar_docking.md`](pr_proposals/04_canvas_gutter_and_quick_bar_docking.md) | `crates/ui-egui` | **Variation & UX Enhancement** | Resolves canvas dead space and document overlap |

---

## How to Submit These to Upstream

1. **Verify your local branch:**
   ```bash
   git status
   git diff
   ```
2. **Commit with clean conventional commit messages:**
   - `fix(ui): enlarge and tile tool row icons with crisp stroke rendering`
   - `fix(edit): guard path segment operators before m in image scan (#3581)`
   - `fix: read the last array of a surplus-operand TJ, not the first (#3569)`
   - `fix(measure): join /Contents pieces with space instead of newline in snap (#3570)`
   - `fix(edit): separate paragraphs across blank lines and headings (#3577)`
   - `fix(print): restore previous paper size after leaving Label mode (#3564)`
   - `fix(canvas): test exact QuadPoints for text markup annotation hover popups (#3541, #3620)`
   - `fix(edit): track excess marked content depth symmetrically (#3573)`
   - `fix(automation): normalize angle to quarter turns to avoid integer overflow (#3187)`
   - `fix(ui): back off on failed accept in control channel listener (#3631)`
   - `fix(ui): prevent preferences scrollbar from overflowing theme card and OK button (#3557)`
   - `fix(organize): iteratively prune name trees to prevent stack overflow (#3309)`
   - `fix(forms): provision default AcroForm fonts as indirect references for Adobe Reader (#3441)`
3. **Push to your GitHub fork and open the PRs using the templates provided in `enhancements/pr_proposals/`!**
