# Vendored crates

Permissively licensed crates we carry small patches for. Each directory keeps the upstream
licence files unchanged. Every patch is marked `PrintCraft patch:` in the source, is covered by a
PrintCraft test, and should be proposed upstream. Remove the vendored copy once upstream releases
the fix. Vendoring copyleft code is never allowed (plan/adr/0001).

| Crate | Version | Licence | Patches | Test |
|---|---|---|---|---|
| hayro-interpret | 0.7.0 | Apache-2.0 OR MIT | (1) select `/AP /N` appearance states via `/AS` (checkboxes, radios, stateful icons) and honour the NoView flag; (2) Type3 Unicode fallback to `/Encoding` glyph names, then printable ASCII; (3) `Type3Glyph::advance_width`; (4) `InterpreterSettings::ocg_overrides` for viewer layer toggles; (5) soft masks whose group lacks `/CS` were dropped (Chrome gradient text); (6) paint nesting cap `MAX_PAINT_NESTING` for tiling patterns and Type 3 glyphs (self-referencing content overflowed the stack); (7) CID width ranges in `/W` and `/W2` clamped to `MAX_CID` (a `0 4294967295 w` range hung); (8) `InterpreterSettings::hide_comments` skips markup annotations but keeps widgets and links (View ▸ Hide all comments) | (1) `raster::tests::widget_appearance_states`; (2, 3) `xtask text-oracle` on corpus/pdfjs issue918; (4) `raster::tests::layer_override_hides_content`; (5) `raster::tests::soft_mask_without_group_colour_space_is_applied`; (6) `raster::tests::self_referencing_tiling_pattern_terminates`, `raster::tests::self_referencing_type3_glyph_terminates`; (7) `raster::tests::huge_cid_width_ranges_terminate`; (8) `raster::tests::hiding_comments_keeps_fields` |

| hayro | 0.7.1 | Apache-2.0 OR MIT | (1) `RenderSettings::{x_offset, y_offset}` to render one tile of a large page; (2) images over `MAX_IMAGE_PIXELS` (2^28) are skipped (a fuzzed `/W 4294967295` hung in resampling); (3) `tiling_cell_scale` keeps a tiling pattern's cell pixmap within 3000 px a side whatever its `/XStep`/`/YStep` (a fuzzed step far beyond `/BBox` allocated 2 GB) | (1) `raster::tests::tiles_match_full_render`; (2) `raster::tests::absurd_image_dimensions_are_skipped`; (3) `raster::tests::tiling_cells_stay_small_whatever_the_step` |
| lopdf | 0.45.0 | MIT | (1) PNG predictor rows longer than the data are refused before their buffers are allocated (a fuzzed `/Columns 4294967295` allocated two 4 GiB rows); (2) predictor row widths use saturating arithmetic | (1) `inspect::tests::png_predictor_rows_longer_than_the_data_are_refused_before_allocating` |
| hayro-syntax | 0.7.2 | Apache-2.0 OR MIT | Page-tree cycle guard in `resolve_pages` (visited set + depth cap): a `/Kids` loop among compressed objects overflowed the stack and aborted the process | `cos/tests/writer.rs::page_tree_loops_in_object_streams_do_not_crash_readers` |

Temporary: hayro is the bootstrap renderer and lopdf the bootstrap inspector (ADR-0004); M2.6 replaces them with our own devices and `cos`/`model`.
