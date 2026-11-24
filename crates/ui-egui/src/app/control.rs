use super::{
    DocView, Mode, PdfCraftApp, PropsTab, QuickTool, RightPanel, canvas, combine_ui, comments, dialogs, export_ui, fill_sign, i18n, measure_ui,
    prepare, scroll_speed, ui_scale,
};

impl PdfCraftApp {
    /// Enable the UI control channel (opt-in; see [`control`]): `channel` receives the requests,
    /// and `listening` is the loopback server that feeds it (`None` when something else feeds it).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn attach_control(&mut self, channel: craft::automation::ControlChannel, listening: Option<craft::automation::server::Listening>) {
        self.control = Some(channel);
        self.control_server = listening;
    }

    /// Enable the UI control channel (see [`control`]): `channel` receives the page's requests
    /// (`window.pdfcraft`); the web has no loopback server.
    #[cfg(target_arch = "wasm32")]
    pub fn attach_control(&mut self, channel: craft::automation::ControlChannel) {
        self.control = Some(channel);
    }

    /// Apply a named view option (`--page 3`, `--panel bookmarks`, `--theme dark`, …).
    ///
    /// This is the seed of the UI control channel (M3.9): the same verbs become `ui.set` calls.
    pub fn set_option(&mut self, ctx: &egui::Context, key: &str, value: &str) -> Result<(), String> {
        let view = self.active.and_then(|i| self.views.get_mut(i));
        match (key, view) {
            ("hotbar-tools", _) => self.hotbar.set_tools(value)?,
            ("language", _) => {
                let language = i18n::normalize_pref(value).ok_or_else(|| {
                    let codes: Vec<&str> = std::iter::once(i18n::AUTO).chain(i18n::Lang::all().map(i18n::Lang::code)).collect();
                    format!("language must be one of {}", codes.join(", "))
                })?;
                self.language = language.to_string();
            }
            ("theme", _) => {
                let preference = match value {
                    "system" => craft::theme::ModeChoice::System,
                    "light" => craft::theme::ModeChoice::Light,
                    "dark" => craft::theme::ModeChoice::Dark,
                    _ => return Err("theme must be light, dark, or system".into()),
                };
                self.set_theme_preference(preference);
            }
            ("show_page_edge_shadows", _) => {
                self.show_page_edge_shadows = value.parse::<bool>().map_err(|_| "show_page_edge_shadows must be true or false")?;
            }
            ("update_check_at_start", _) => {
                self.update_check_at_start = value.parse::<bool>().map_err(|_| "update_check_at_start must be true or false")?;
            }
            ("scroll_speed", _) => {
                self.scroll_speed = scroll_speed::ScrollSpeed::parse(value).ok_or("scroll_speed must be slow, normal, or fast")?;
            }
            ("invert_middle_drag", _) => {
                self.invert_middle_drag = value.parse::<bool>().map_err(|_| "invert_middle_drag must be true or false")?;
            }
            ("animation_effects", _) => {
                // PLAN: assumed `crate::settings::motion_from_name(&str) -> Option<MotionChoice>` and `set_motion(&mut self, MotionChoice)`, which writes the field and the store (pdf-shell-w9).
                // The control channel and the command line take the names in any case, as before the port.
                let motion = crate::settings::motion_from_name(&value.to_ascii_lowercase()).ok_or("animation_effects must be system, on, or off")?;
                self.set_motion(motion);
            }
            ("panel", _) => {
                let panel = match value {
                    "comments" => Some(RightPanel::Comments),
                    "bookmarks" => Some(RightPanel::Bookmarks),
                    "pages" => Some(RightPanel::Pages),
                    "fields" => Some(RightPanel::Fields),
                    "layers" => Some(RightPanel::Layers),
                    "attachments" => Some(RightPanel::Attachments),
                    "signatures" => Some(RightPanel::Signatures),
                    "accessibility" => Some(RightPanel::Accessibility),
                    "tags" => Some(RightPanel::Tags),
                    "search" => Some(RightPanel::Search),
                    "compare" => Some(RightPanel::Compare),
                    "none" => None,
                    other => return Err(format!("unknown panel {other}")),
                };
                self.set_right(panel);
            }
            ("mode", _) => {
                let mode = Mode::parse(value).unwrap_or(Mode::AllTools);
                self.mode_override = Some(mode);
                self.mode = mode;
                // As switch_mode: another workspace puts the expanded All tools view away.
                if mode != Mode::AllTools {
                    self.all_tools_full = false;
                }
            }
            ("default-mode", _) => {
                self.default_mode = Mode::parse(value).ok_or("default-mode must be all, read, edit, convert or sign")?;
            }
            ("date-format", _) => {
                self.session.set_date_format(value)?;
                self.sync_open_date_format();
            }
            ("date-language", _) => {
                self.session.set_date_language(Some(value).filter(|v| *v != "auto"))?;
                self.sync_open_date_language();
            }
            ("tool", _) => {
                let g = pdfcraft_engine::catalog::group(value).ok_or_else(|| format!("unknown tool {value}"))?;
                self.select_tool(ctx, g.id);
            }
            ("left", _) => self.set_left_open(value != "closed"),
            ("home", _) => {
                self.active = None;
                // With no document open the expanded All tools view covers Home itself, so no
                // other tab comes forward to put it away.
                self.all_tools_full = false;
            }
            // Discarded first: `sign` and `certify` start a draft that the discard would wipe.
            ("dialog", _) => {
                dialogs::discard_inactive_password_drafts(self);
                self.open_dialog_named(ctx, value)?;
            }
            ("tools", _) if value == "full" => self.expand_all_tools(ctx),
            ("tools", _) => {
                self.all_tools_full = false;
                self.all_tools_expanded = value != "collapsed";
            }
            // The palette opens empty: typing reaches it through the runtime's input, as a user's.
            ("palette", _) => self.palette_open = true,
            ("page", Some(v)) => v.go_to_page(value.parse::<usize>().map_err(|e| e.to_string())?.saturating_sub(1)),
            ("zoom", Some(v)) => {
                let zoom = DocView::parse_zoom_percent(value).ok_or("zoom must be a percentage above 0")?;
                v.view.zoom = f64::from(zoom);
                v.view.fit = canvas::Fit::None;
            }
            ("layout", Some(v)) => {
                let (display, continuous) = canvas::parse_display_token(value).ok_or("layout must be single, continuous, two-up or two-page")?;
                (v.view.display, v.view.continuous) = (display, continuous);
            }
            ("continuous", Some(v)) => match value {
                "on" => v.view.continuous = true,
                "off" => v.view.continuous = false,
                _ => return Err("continuous must be on or off".into()),
            },
            ("page-gaps", Some(_)) => {
                let show = match value {
                    "on" => true,
                    "off" => false,
                    _ => return Err("page-gaps must be on or off".into()),
                };
                self.set_active_page_gaps(show);
            }
            ("full-screen-escape", _) => {
                self.full_screen_escape_exits = match value {
                    "on" => true,
                    "off" => false,
                    _ => return Err("full-screen-escape must be on or off".into()),
                };
            }
            ("full-screen-fill", _) => {
                self.full_screen_fill = match value {
                    "on" => true,
                    "off" => false,
                    _ => return Err("full-screen-fill must be on or off".into()),
                };
            }
            ("default-page-gaps", _) => {
                self.view_defaults.page_gaps = match value {
                    "on" => true,
                    "off" => false,
                    _ => return Err("default-page-gaps must be on or off".into()),
                };
            }
            ("recent-files", _) => {
                let max = crate::RECENT_FILES_MAX;
                let limit = value.parse().ok().filter(|n| *n <= max).ok_or_else(|| format!("recent-files must be between 0 and {max}"))?;
                self.recent_limit = limit;
                self.recent.truncate(limit);
            }
            ("cover", Some(v)) => {
                let on = match value {
                    "on" => true,
                    "off" => false,
                    _ => return Err("cover must be on or off".into()),
                };
                if on && !v.cover_applies() {
                    return Err("switch to two-page view first to show the cover page".into());
                }
                v.view.cover = on;
            }
            ("default-layout", _) => {
                let (display, continuous) =
                    canvas::parse_display_token(value).ok_or("default-layout must be single, continuous, two-up or two-page")?;
                (self.view_defaults.display, self.view_defaults.continuous) = (display, continuous);
            }
            ("default-continuous", _) => {
                self.view_defaults.continuous = match value {
                    "on" => true,
                    "off" => false,
                    _ => return Err("default-continuous must be on or off".into()),
                };
            }
            ("default-zoom", _) => {
                self.view_defaults =
                    self.view_defaults.with_zoom(value).ok_or("default-zoom must be fit-width, fit-page or a percentage from 8 to 6400")?;
            }
            ("always-default-view", _) => {
                self.always_default_view = match value {
                    "on" => true,
                    "off" => false,
                    _ => return Err("always-default-view must be on or off".into()),
                };
            }
            ("tools-side", _) => {
                let edge = side_name(value, "tools-side")?;
                crate::side_panels::command(self, false, &serde_json::json!({ "panel": "tools", "edge": edge }))?;
            }
            ("rail-side", _) => {
                let side = side_name(value, "rail-side")?;
                crate::side_panels::command(self, false, &serde_json::json!({ "rail_side": side }))?;
            }
            ("organize", Some(v)) => {
                if value == "off" {
                    v.organize = false;
                } else {
                    v.open_organize();
                }
            }
            // Inspector thumbnail size, in percent, independent of document and grid zoom.
            ("thumbnail-zoom", Some(v)) => {
                let percent = value.trim_end_matches('%').parse::<f64>().map_err(|e| e.to_string())?;
                if !percent.is_finite() || !craft::pages::ui::THUMBNAIL_ZOOM.range().contains(&(percent / 100.0)) {
                    return Err("thumbnail-zoom must be between 50 and 150".into());
                }
                v.set_thumbnail_zoom((percent / 100.0) as f32);
            }
            // `--grid-zoom 150`: the size of the pages in the organize grid, in percent. The
            // grid's size is one for every document, as it is remembered.
            ("grid-zoom", Some(_)) => {
                let percent = value.trim_end_matches('%').parse::<f64>().map_err(|e| e.to_string())?;
                if !percent.is_finite() || !craft::pages::ui::ORGANIZE_ZOOM.range().contains(&(percent / 100.0)) {
                    return Err("grid-zoom must be between 50 and 300".into());
                }
                // PLAN: assumed `organize_zoom: f64` (CV-C27, pdf-hub-w1).
                self.organize_zoom = percent / 100.0;
            }
            ("rotate", Some(v)) => {
                let deg: u16 = value.parse().map_err(|_| "rotate: 0, 90, 180 or 270")?;
                if !deg.is_multiple_of(90) {
                    return Err("rotate: 0, 90, 180 or 270".into());
                }
                v.view.rotation = deg % 360;
            }
            ("layer", _) => {
                // `--layer "Name=off"` / `"Name=on"`
                let (name, state) = value.rsplit_once('=').ok_or("expected NAME=on|off")?;
                let (i, id) = self.active_ids().ok_or("`layer` needs an open document")?;
                let idx = self
                    .session
                    .get(id)
                    .and_then(|d| d.info.layers.iter().position(|l| l.name == name))
                    .ok_or_else(|| format!("no layer named {name}"))?;
                if self.session.set_layer_visible(id, idx, state != "off") {
                    self.views[i].invalidate_content();
                }
            }
            ("find", Some(v)) => {
                v.open_find();
                if let Some(f) = v.find.as_mut() {
                    f.query = value.to_string();
                }
                v.rerun_find();
            }
            ("fields", Some(v)) => v.session.highlight_fields = value != "off",
            ("select", Some(v)) => {
                // The same physical page ranges as the Organize Pages selection field.
                let pages = craft::pages::parse_pages(value, v.page_count()).map_err(|e| e.to_string())?;
                // PLAN: assumed `select_pages(&mut self, &[usize])` builds the view's `Selection` (pdf-chrome-w17, canvas-03 :68).
                v.select_pages(&pages);
            }
            // `--select-text 2:10-40`: characters 10 to 40 of page 2 selected, as a drag would.
            ("select-text", Some(v)) => {
                const USAGE: &str = "select-text: PAGE:FROM-TO";
                let (page, span) = value.split_once(':').ok_or(USAGE)?;
                let (from, to) = span.split_once('-').ok_or(USAGE)?;
                let number = |text: &str| text.trim().parse::<usize>().map_err(|_| USAGE);
                let page = number(page)?.checked_sub(1).filter(|page| *page < v.page_count()).ok_or("select-text: no such page")?;
                v.select_text(page, number(from)?, number(to)?);
            }
            // `--prepare-select NAME[:WIDGET]`: one form field selected in Prepare a form.
            ("prepare-select", Some(v)) => {
                let (name, widget) = match value.rsplit_once(':') {
                    Some((name, w)) => (name, w.parse::<usize>().map_err(|_| "prepare-select: NAME or NAME:WIDGET")?),
                    None => (value, 0),
                };
                v.session.prepare.selected = Some((name.to_string(), widget));
            }
            // `progress-hold 0.4` shows the card of a held fake job at that fraction, with Cancel;
            // `null` takes it away.
            ("progress-hold", _) => {
                use crate::notices::{ProgressKey, ProgressNotice, end_progress, set_progress};
                if value == "null" {
                    end_progress(self, ProgressKey::Export);
                } else {
                    let fraction = value.parse::<f32>().map_err(|_| "progress-hold must be a fraction or null")?;
                    set_progress(
                        self,
                        ProgressKey::Export,
                        ProgressNotice { label: tl!("Held for a capture").to_owned(), fraction, cancellable: true },
                    );
                }
            }
            // `--recent-colours ff0000,00ff00`: the custom colours the pickers remember, as hex, the
            // first ending up newest.
            ("recent-colours", _) => {
                let hex = |text: &str| -> Result<[f64; 3], String> {
                    let digits = text.trim().trim_start_matches('#');
                    let channel = |at: usize| digits.get(at..at + 2).and_then(|pair| u8::from_str_radix(pair, 16).ok()).map(|v| f64::from(v) / 255.0);
                    match (digits.len(), channel(0), channel(2), channel(4)) {
                        (6, Some(r), Some(g), Some(b)) => Ok([r, g, b]),
                        _ => Err(format!("recent-colours: `{text}` is not a hex colour (rrggbb)")),
                    }
                };
                let colours = value.split(',').filter(|text| !text.trim().is_empty()).map(hex).collect::<Result<Vec<_>, _>>()?;
                for colour in colours.into_iter().rev() {
                    self.recent_colours.remember(colour);
                }
            }
            ("notice", Some(v)) => v.notice_dismissed = value == "off",
            ("quick", _) => {
                // `--quick select|hand|note|freetext|highlight|underline|strikeout|ink|line|arrow|square|circle`
                self.quick_tool = match value {
                    "select" => QuickTool::Select,
                    measure if measure.starts_with("measure-") => QuickTool::Measure(
                        measure_ui::Tool::from_name(measure.trim_start_matches("measure-")).ok_or_else(|| format!("unknown tool {measure}"))?,
                    ),
                    "hand" => QuickTool::Hand,
                    "crop" => QuickTool::Crop,
                    "enhance-camera" => QuickTool::EnhanceCamera,
                    "redact" => QuickTool::Redact,
                    "add-text" => QuickTool::AddText,
                    "edit-text" => QuickTool::EditText,
                    "link" => QuickTool::Link,
                    "sign" => QuickTool::SignArea { certify: false },
                    "marquee-zoom" => QuickTool::MarqueeZoom,
                    "snapshot" => QuickTool::Snapshot,
                    "column-select" => QuickTool::ColumnSelect,
                    "certify" => QuickTool::SignArea { certify: true },
                    custom if custom.starts_with("custom-stamp-") => {
                        let i: usize = custom[13..].parse().map_err(|_| format!("bad stamp {custom}"))?;
                        if i >= self.custom_stamps.len() {
                            return Err(format!("there are {} custom stamps", self.custom_stamps.len()));
                        }
                        QuickTool::CustomStamp(i)
                    }
                    stamp if stamp.starts_with("stamp-") => QuickTool::Stamp(
                        pdfcraft_engine::StampKind::ALL
                            .into_iter()
                            .find(|k| k.name().trim_start_matches("PC").eq_ignore_ascii_case(&stamp[6..]))
                            .ok_or_else(|| format!("unknown stamp {stamp}"))?,
                    ),
                    field if field.starts_with("field-") => QuickTool::Field(
                        prepare::FieldTool::from_command(&format!("form.add.{}", &field[6..])).ok_or_else(|| format!("unknown tool {field}"))?,
                    ),
                    fill if fill.starts_with("fill-") => {
                        let tool =
                            fill_sign::FillTool::from_command(&format!("sign.fill.{}", &fill[5..])).ok_or_else(|| format!("unknown tool {fill}"))?;
                        self.fill_tool = tool;
                        QuickTool::Fill(tool)
                    }
                    other => {
                        let t = comments::CommentTool::from_command(&format!("comment.{other}")).ok_or_else(|| format!("unknown tool {other}"))?;
                        self.comment_prefs.group_tool[t.group()] = t;
                        QuickTool::Comment(t)
                    }
                };
            }
            ("author", _) => self.comment_prefs.author = value.to_string(),
            ("folder-collapsed", _) => self.set_folder_collapsed(value)?,
            ("comment-tool-pinned", _) => {
                self.comment_prefs.pinned = match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err("comment-tool-pinned must be true or false".into()),
                };
            }
            ("hide-empty-highlight-hover", _) => {
                self.comment_prefs.hide_empty_highlight_hover = match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err("hide-empty-highlight-hover must be true or false".into()),
                };
            }
            ("text-links", _) => {
                self.text_links = match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err("text-links must be true or false".into()),
                };
            }
            ("ui-scale", _) => self.ui_scale = ui_scale::parse(value)?,
            // `--secured-notice false`: no notice above the pages that a document is secured.
            ("secured-notice", _) => {
                self.show_secured_notice = match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err("secured-notice must be true or false".into()),
                };
            }
            ("system-title-bar", _) => {
                self.title_bar_choice = match value {
                    "true" => craft::window::policy::TitleBarPref::System,
                    "false" => craft::window::policy::TitleBarPref::Integrated,
                    _ => return Err("system-title-bar must be true or false".into()),
                };
            }
            ("flatten-fill-sign", _) => {
                self.flatten_fill_sign_on_save = match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err("flatten-fill-sign must be true or false".into()),
                };
            }
            ("comment", Some(v)) => {
                // `--comment 2:4` selects the 4th annotation of page 2 (1-based, as comment_list reports).
                let (p, i) = value.split_once(':').ok_or("comment: PAGE:INDEX")?;
                let (p, i): (usize, usize) =
                    (p.trim().parse().map_err(|_| "comment: PAGE:INDEX")?, i.trim().parse().map_err(|_| "comment: PAGE:INDEX")?);
                let thread = (p.saturating_sub(1), i.saturating_sub(1));
                v.session.comments.select_only(thread);
                v.session.comments.reveal = true;
            }
            // `--combine-view list`: Combine files as thumbnails (grid) or as the table (list).
            ("combine-view", _) => {
                self.combine_view = combine_ui::CombineView::parse(value).ok_or("combine-view must be grid or list")?;
            }
            // `--combine-zoom 150`: the size of the cards in the Combine grid, in percent.
            ("combine-zoom", _) => {
                let range = craft::ui::tile_grid::TileZoom::Ladder.range();
                let percent = value.trim_end_matches('%').parse::<f64>().map_err(|e| e.to_string())?;
                if !percent.is_finite() || !range.contains(&(percent / 100.0)) {
                    return Err(format!("combine-zoom must be between {} and {}", range.start() * 100.0, range.end() * 100.0));
                }
                self.set_combine_zoom(percent / 100.0);
            }
            (k, None)
                if [
                    "page",
                    "zoom",
                    "layout",
                    "continuous",
                    "page-gaps",
                    "cover",
                    "organize",
                    "grid-zoom",
                    "thumbnail-zoom",
                    "fields",
                    "find",
                    "rotate",
                    "select",
                    "select-text",
                    "prepare-select",
                    "notice",
                    "comment",
                ]
                .contains(&k) =>
            {
                return Err(format!("`{k}` needs an open document"));
            }
            (other, _) => return Err(format!("unknown option {other}")),
        }
        Ok(())
    }

    /// `ui.set dialog <name>`: opens the dialog through the same opener its command uses. `none`
    /// cancels every host dialog and both inline windows; an unknown name opens About.
    // PLAN: assumed `cancel_dialogs(&mut self)` (pdf-hub-w2) and `prepare::dialog::open(&mut PdfCraftApp, &str, usize, FieldTab)` (L-105).
    fn open_dialog_named(&mut self, ctx: &egui::Context, value: &str) -> Result<(), String> {
        use pdfcraft_engine::MarkKind;
        let (value, field) = match value.split_once(':') {
            Some((value, field)) => (value, Some(field)),
            None => (value, None),
        };
        match value {
            "properties" => self.open_properties(ctx, PropsTab::Description),
            "security" => self.open_properties(ctx, PropsTab::Security),
            "fonts" => self.open_properties(ctx, PropsTab::Fonts),
            "custom" => self.open_properties(ctx, PropsTab::Custom),
            "advanced" => self.open_properties(ctx, PropsTab::Advanced),
            "space-audit" => self.open_properties(ctx, PropsTab::SpaceAudit),
            "shortcuts" => self.open_shortcuts(ctx),
            "split" => self.open_split(ctx, false),
            "protect" => self.open_protect(),
            "page-boxes" => self.open_page_boxes(ctx, false),
            "header-footer" => self.open_marks(ctx, MarkKind::HeaderFooter, false),
            "watermark" => self.open_marks(ctx, MarkKind::Watermark, false),
            "background" => self.open_marks(ctx, MarkKind::Background, false),
            "printer-marks" => self.open_printer_marks(ctx),
            "export-image" => export_ui::open(self, ctx, export_ui::ExportKind::Image),
            "word-count" => self.open_word_count(),
            "export-text" => export_ui::open(self, ctx, export_ui::ExportKind::Text),
            "export-all-images" => export_ui::open(self, ctx, export_ui::ExportKind::AllImages),
            "accessibility-options" => crate::a11y_ui::open_a11y_options(self, ctx),
            "recognize-text" => self.open_recognize_text(ctx),
            "enhance-scan" => self.open_enhance_scan(ctx),
            "js-console" => crate::js_dialogs::open_console(self),
            "document-js" => crate::js_dialogs::open_document_scripts(self),
            "preferences" => self.open_preferences(),
            "compare-files" => crate::compare_ui::open_compare_dialog(self, ctx),
            "action-wizard" => crate::actions_ui::open_wizard(self, ctx, None),
            "pdfa" => self.open_pdfa(),
            "signature" => {
                let initials = field == Some("initials");
                let editing = if initials { self.initials.is_some() } else { self.signature.is_some() };
                crate::dialogs::signature::open(self, initials, editing);
            }
            "optimize" => crate::optimize_ui::open_optimizer(self, ctx),
            "sign" | "certify" => {
                // Sign with a Digital ID for an invisible signature on the current page.
                let page = self.active.and_then(|i| self.views.get(i)).map_or(0, |v| v.current());
                self.start_signing(ctx, page, None, None, (value == "certify").then_some(2));
            }
            "number-pages" => self.open_number_pages(ctx),
            "convert-colours" => self.open_convert_colours(ctx),
            // Through the command, so it is refused with no document or none that can be
            // modified, as the menu is.
            "fix-hairlines" => {
                self.execute(ctx, "prepress.hairlines");
            }
            // Same path as the panel, so the selected pages and their order are seeded.
            "page-properties" => {
                self.execute(ctx, "page.properties");
            }
            "page.rotate_dialog" => {
                self.execute(ctx, "page.rotate_dialog");
            }
            "comment-props" => {
                let (page, index) = self
                    .active
                    .and_then(|i| self.views.get(i))
                    .and_then(|v| v.session.comments.primary())
                    .ok_or("`dialog comment-props` needs a selected comment")?;
                self.open_comment_props(ctx, page, index);
            }
            "field-properties" => {
                let (i, _) = self.active_ids().ok_or("`dialog field-properties` needs an open document")?;
                let (name, widget) = match field {
                    Some(name) => (name.to_string(), 0),
                    None => self.views[i].session.prepare.selected.clone().ok_or("select a field first, or name one: field-properties:NAME")?,
                };
                prepare::dialog::open(self, &name, widget, prepare::FieldTab::General);
            }
            "none" => {
                self.cancel_dialogs(ctx);
                // Preferences is a separate live window, not part of the dialog host.
                self.preferences = None;
                self.redact_props = None;
                self.redact_search_open = false;
                self.hidden_draft = None;
                self.sign_draft = None;
                self.cert_viewer = None;
            }
            "about" => self.open_about(craft::ui::patterns::form_dialogs::AboutTab::default()),
            _ => self.open_about(craft::ui::patterns::form_dialogs::AboutTab::default()),
        }
        Ok(())
    }
}

/// The dock side a control value names (`left` or `right`), as `side_panels::command` takes it.
fn side_name<'a>(value: &'a str, key: &str) -> Result<&'a str, String> {
    match value {
        "left" | "right" => Ok(value),
        _ => Err(format!("{key} must be left or right")),
    }
}
