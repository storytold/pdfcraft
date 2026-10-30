use super::*;
use crate::control;

impl PdfCraftApp {
    /// Enable the UI control channel on `ctx` (opt-in; see [`control`]). Returns a client that
    /// sends requests to this app; [`control::serve`] exposes it on loopback.
    pub fn attach_control(&mut self, ctx: &egui::Context) -> control::ControlClient {
        let (control, client) = control::attach(ctx);
        self.control = Some(control);
        client
    }

    /// Apply a named view option (`--page 3`, `--panel bookmarks`, `--theme dark`, …).
    ///
    /// This is the seed of the UI control channel (M3.9): the same verbs become `ui.set` calls.
    pub fn set_option(&mut self, key: &str, value: &str) -> Result<(), String> {
        let view = self.active.and_then(|i| self.views.get_mut(i));
        match (key, view) {
            ("language", _) => {
                let language = i18n::normalize_pref(value).ok_or_else(|| {
                    let codes: Vec<&str> = std::iter::once(i18n::AUTO).chain(i18n::Lang::all().map(i18n::Lang::code)).collect();
                    format!("language must be one of {}", codes.join(", "))
                })?;
                self.language = language.to_string();
            }
            ("theme", _) => {
                let preference = match value {
                    "system" => ThemePreference::System,
                    "light" => ThemePreference::Light,
                    "dark" => ThemePreference::Dark,
                    _ => return Err("theme must be light, dark, or system".into()),
                };
                self.set_theme_preference(preference);
            }
            ("scroll_speed", _) => {
                self.scroll_speed = scroll_speed::ScrollSpeed::parse(value).ok_or("scroll_speed must be slow, normal, or fast")?;
            }
            ("animation_effects", _) => {
                self.animation_effects = motion::AnimationEffects::parse(value).ok_or("animation_effects must be system, on, or off")?;
            }
            ("panel", _) => {
                self.right = match value {
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
                }
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
            ("date-format", _) => self.session.set_date_format(value)?,
            ("date-language", _) => self.session.set_date_language(Some(value).filter(|v| *v != "auto"))?,
            ("tool", _) => {
                let g = pdfcraft_engine::catalog::group(value).ok_or_else(|| format!("unknown tool {value}"))?;
                self.select_tool(g.id);
            }
            ("left", _) => self.left_open = value != "closed",
            ("home", _) => {
                self.active = None;
                // With no document open the expanded All tools view covers Home itself, so no
                // other tab comes forward to put it away.
                self.all_tools_full = false;
            }
            ("dialog", _) => {
                self.dialog = match value {
                    "properties" => Some(Dialog::Properties(PropsTab::Description)),
                    "security" => Some(Dialog::Properties(PropsTab::Security)),
                    "fonts" => Some(Dialog::Properties(PropsTab::Fonts)),
                    "custom" => Some(Dialog::Properties(PropsTab::Custom)),
                    "advanced" => Some(Dialog::Properties(PropsTab::Advanced)),
                    "space-audit" => Some(Dialog::Properties(PropsTab::SpaceAudit)),
                    "shortcuts" => Some(Dialog::Shortcuts),
                    "split" => Some(Dialog::Split),
                    "protect" => Some(Dialog::Protect),
                    "page-boxes" => Some(Dialog::PageBoxes),
                    "header-footer" => Some(Dialog::Marks(pdfcraft_engine::MarkKind::HeaderFooter)),
                    "watermark" => Some(Dialog::Marks(pdfcraft_engine::MarkKind::Watermark)),
                    "background" => Some(Dialog::Marks(pdfcraft_engine::MarkKind::Background)),
                    "export-image" => Some(Dialog::Export(export_ui::ExportKind::Image)),
                    "word-count" => Some(Dialog::WordCount),
                    "export-text" => Some(Dialog::Export(export_ui::ExportKind::Text)),
                    "export-all-images" => Some(Dialog::Export(export_ui::ExportKind::AllImages)),
                    "export-form-data" => Some(Dialog::ExportData(crate::data_export_ui::DataKind::FormData)),
                    "export-comments" => Some(Dialog::ExportData(crate::data_export_ui::DataKind::Comments)),
                    "accessibility-options" => Some(Dialog::AccessibilityOptions),
                    "recognize-text" => Some(Dialog::RecognizeText),
                    "js-console" => Some(Dialog::JsConsole),
                    "document-js" => Some(Dialog::DocumentJs),
                    "preferences" => Some(Dialog::Preferences),
                    "compare-files" => Some(Dialog::CompareFiles),
                    "action-wizard" => Some(Dialog::ActionWizard),
                    "pdfa" => Some(Dialog::PdfA),
                    "signature" => Some(Dialog::Signature),
                    "optimize" => Some(Dialog::Optimize),
                    "sign" | "certify" => {
                        // Sign with a Digital ID for an invisible signature on the current page.
                        let page = self.active.map_or(0, |i| self.views[i].current);
                        self.start_signing(page, None, None, (value == "certify").then_some(2));
                        Some(Dialog::Sign)
                    }
                    "number-pages" => {
                        // Same path as the menu, so the page range is seeded.
                        self.execute("page.number");
                        Some(Dialog::NumberPages)
                    }
                    "page-properties" => {
                        // Same path as the panel, so the selected pages and their order are seeded.
                        self.execute("page.properties");
                        self.dialog
                    }
                    "none" => None,
                    _ => Some(Dialog::About),
                }
            }
            ("tools", _) if value == "full" => self.expand_all_tools(),
            ("tools", _) => {
                self.all_tools_full = false;
                self.all_tools_expanded = value != "collapsed";
            }
            ("palette", _) => {
                self.palette_open = true;
                self.palette_query = value.to_string();
            }
            ("page", Some(v)) => v.go_to_page(value.parse::<usize>().map_err(|e| e.to_string())?.saturating_sub(1)),
            ("zoom", Some(v)) => v.set_zoom(DocView::parse_zoom_percent(value).ok_or("zoom must be a percentage above 0")?),
            ("layout", Some(v)) => {
                let (layout, continuous) = canvas::parse_display_token(value).ok_or("layout must be single, continuous, two-up or two-page")?;
                v.set_display(layout, continuous);
            }
            ("continuous", Some(v)) => match value {
                "on" => v.set_continuous(true),
                "off" => v.set_continuous(false),
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
            ("default-page-gaps", _) => {
                self.view_defaults.show_page_gaps = match value {
                    "on" => true,
                    "off" => false,
                    _ => return Err("default-page-gaps must be on or off".into()),
                };
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
                v.set_cover(on);
            }
            ("default-layout", _) => {
                let (layout, continuous) =
                    canvas::parse_display_token(value).ok_or("default-layout must be single, continuous, two-up or two-page")?;
                (self.view_defaults.layout, self.view_defaults.continuous) = (layout, continuous);
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
                self.side_panels.tools_side = match value {
                    "left" => side_panels::ToolsSide::Left,
                    "right" => side_panels::ToolsSide::Right,
                    _ => return Err("tools-side must be left or right".into()),
                }
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
                let percent = value.trim_end_matches('%').parse::<f32>().map_err(|e| e.to_string())?;
                if !percent.is_finite() || !canvas::THUMBNAIL_ZOOM_RANGE.contains(&(percent / 100.0)) {
                    return Err("thumbnail-zoom must be between 50 and 150".into());
                }
                v.set_thumbnail_zoom(percent / 100.0);
            }
            // `--grid-zoom 150`: the size of the pages in the organize grid, in percent.
            ("grid-zoom", Some(v)) => {
                let percent = value.trim_end_matches('%').parse::<f32>().map_err(|e| e.to_string())?;
                if !percent.is_finite() || !canvas::GRID_ZOOM_RANGE.contains(&(percent / 100.0)) {
                    return Err("grid-zoom must be between 50 and 300".into());
                }
                v.set_grid_zoom(percent / 100.0);
                // The grid's size is one for every document, as it is remembered.
                self.organize_zoom = v.grid_zoom();
            }
            ("rotate", Some(v)) => {
                let deg: u16 = value.parse().map_err(|_| "rotate: 0, 90, 180 or 270")?;
                if !deg.is_multiple_of(90) {
                    return Err("rotate: 0, 90, 180 or 270".into());
                }
                v.rotation = deg % 360;
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
            ("fields", Some(v)) => v.highlight_fields = value != "off",
            ("select", Some(v)) => {
                // The same physical page ranges as the Organize Pages selection field.
                let pages = crate::page_selection_ui::parse(value, v.page_count()).map_err(|e| e.to_string())?;
                v.select_pages(&pages);
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
                    fill if fill.starts_with("fill-") => QuickTool::Fill(
                        fill_sign::FillTool::from_command(&format!("sign.fill.{}", &fill[5..])).ok_or_else(|| format!("unknown tool {fill}"))?,
                    ),
                    other => {
                        let t = comments::CommentTool::from_command(&format!("comment.{other}")).ok_or_else(|| format!("unknown tool {other}"))?;
                        self.comment_prefs.group_tool[t.group()] = t;
                        QuickTool::Comment(t)
                    }
                };
            }
            ("author", _) => self.comment_prefs.author = value.to_string(),
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
                self.system_title_bar = match value {
                    "true" => true,
                    "false" => false,
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
                v.comments.selected = Some((p.saturating_sub(1), i.saturating_sub(1)));
                v.comments.reveal = true;
            }
            // `--combine-view list`: Combine files as thumbnails (grid) or as the table (list).
            ("combine-view", _) => {
                self.combine_view = combine_ui::CombineView::parse(value).ok_or("combine-view must be grid or list")?;
            }
            // `--combine-zoom 150`: the size of the cards in the Combine grid, in percent.
            ("combine-zoom", _) => {
                let percent = value.trim_end_matches('%').parse::<f32>().map_err(|e| e.to_string())?;
                if !percent.is_finite() || !combine_grid::ZOOM_RANGE.contains(&(percent / 100.0)) {
                    let (lo, hi) = (combine_grid::ZOOM_RANGE.start(), combine_grid::ZOOM_RANGE.end());
                    return Err(format!("combine-zoom must be between {} and {}", lo * 100.0, hi * 100.0));
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
}
