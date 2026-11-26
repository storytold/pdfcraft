use craft::ui::contract::BodyCx;

use super::bookmarks::BmAction;
use super::{Nav, PdfCraftApp, RightPanel};

/// What a side panel's body asked for. The post-pass of [`panel_body`] applies it, so one pass makes one edit.
#[derive(Default)]
pub(crate) struct PanelResult {
    /// Where the view goes.
    pub nav: Option<Nav>,
    /// One engine edit.
    pub edit: Option<pdfcraft_engine::Edit>,
    /// A bookmark action.
    pub bookmark: Option<BmAction>,
    /// The form field and widget whose properties open.
    pub field_properties: Option<(String, usize)>,
    /// An attachment and whether to open it instead of saving it.
    pub attachment: Option<(usize, bool)>,
    /// A layer and its new visibility.
    pub layer: Option<(usize, bool)>,
    /// A command to run, by id.
    pub command: Option<&'static str>,
}

/// Draw the body of side panel `panel` and apply its navigation, layer, bookmark or edit actions. The dock shows the
/// panel's tab, header and menu.
pub fn panel_body(app: &mut PdfCraftApp, ui: &mut egui::Ui, cx: &mut BodyCx<'_>, panel: RightPanel) {
    let ctx = ui.ctx().clone();
    let Some((index, id)) = app.active_ids() else { return };
    // PLAN: the replies line of Comments and the "{n} pages selected" note of Pages are the first line of those
    // bodies (L-86): `comments_panel::show` (pdf-comments-w12) and `pages::body` (pdf-chrome-w14) draw them.
    let mut result = match panel {
        RightPanel::Bookmarks => super::bookmarks::body(app, ui),
        RightPanel::Pages => super::pages::body(app, ui),
        RightPanel::Layers => super::layers::show(app, ui),
        RightPanel::Attachments => super::attachments::show(app, ui, cx),
        RightPanel::Fields => super::fields::show(app, ui),
        RightPanel::Search => super::search_ui::show(app, ui),
        _ => PanelResult::default(),
    };
    let mut sig_action: Option<crate::sign_ui::PanelAction> = None;
    let mut a11y_action: Option<crate::a11y_ui::PanelAction> = None;
    let mut compare_action: Option<crate::compare_ui::PanelAction> = None;
    {
        let Some(doc) = app.session.get(id) else { return };
        let view = &mut app.views[index];
        let prefs = &app.comment_prefs;
        let sig_open = &mut app.sig_open;
        let a11y = &mut app.a11y;
        let tags_state = &mut app.tags_state;
        let compare = &app.compare;
        let comment_allowed = doc.allows_annotation();
        match panel {
            RightPanel::Comments => {
                if let Some(e) = crate::comments_panel::show(ui, doc, view, prefs, comment_allowed, &mut result.nav) {
                    result.edit = Some(e);
                }
            }
            RightPanel::Signatures => {
                sig_action = crate::sign_ui::panel(ui, &doc.signatures, sig_open);
            }
            RightPanel::Accessibility => {
                a11y_action = crate::a11y_ui::panel(ui, a11y, id);
            }
            RightPanel::Tags => {
                let tags = tags_state.tags(doc);
                let content = tags_state.content(doc);
                let (page, edit) = crate::tags_ui::panel(ui, id, &tags, &content, tags_state, doc.allows_modification());
                if let Some(page) = page {
                    result.nav = Some(Nav::Page(page));
                }
                result.edit = edit;
            }
            RightPanel::Compare => compare_action = crate::compare_ui::panel(ui, compare, id),
            _ => {}
        }
    }
    if let Some(e) = result.edit {
        app.apply_edit(ui.ctx(), e);
    }
    if let Some((name, widget)) = result.field_properties {
        crate::prepare::dialog::open(app, &name, widget, crate::prepare::FieldTab::General);
    }
    if let Some(a) = a11y_action {
        app.a11y_action(&ctx, index, a);
    }
    if let Some(a) = compare_action {
        app.compare_action(ui.ctx(), index, a);
    }
    match sig_action {
        Some(crate::sign_ui::PanelAction::Validate) => app.run_command(&ctx, "sign.validate"),
        Some(crate::sign_ui::PanelAction::GoTo(p)) => app.views[index].go_to_page(p),
        Some(crate::sign_ui::PanelAction::Trust(c)) => app.trust_certificate(*c),
        Some(crate::sign_ui::PanelAction::ViewCertificate(chain)) => crate::sign_ui::cert_viewer::open(app, chain),
        Some(crate::sign_ui::PanelAction::ViewSigned { len, revision }) => app.view_signed_version(len, revision),
        Some(crate::sign_ui::PanelAction::Sign(field)) => {
            let page = app.views[index].current();
            app.start_signing(&ctx, page, None, Some(field), None);
        }
        Some(crate::sign_ui::PanelAction::ExportCertificate(c)) => {
            app.export_certificate(&c);
        }
        None => {}
    }
    if let Some((p, i)) = app.views.get_mut(index).and_then(|v| v.session.comments.default_request.take()) {
        app.make_comment_default(p, i);
    }
    if let Some((p, i, open)) = app.views.get_mut(index).and_then(|v| v.session.comments.attachment_request.take()) {
        app.comment_attachment_action(ui.ctx(), id, p, i, open);
    }
    if let Some((p, i)) = app.views.get_mut(index).and_then(|v| v.session.comments.props_request.take()) {
        app.open_comment_props(&ctx, p, i);
    }
    if let Some(text) = app.views.get_mut(index).and_then(|v| v.session.comments.copy_text_request.take()) {
        app.put_text(&text);
    }
    if let Some(command) = result.command {
        app.run_command(&ctx, command);
    }
    if let Some(a) = result.bookmark {
        app.bookmark_action(ui.ctx(), a);
    }
    if let Some((ai, open)) = result.attachment {
        app.attachment_action(ui.ctx(), id, ai, open);
    }
    if let Some((layer, visible)) = result.layer
        && app.session.set_layer_visible(id, layer, visible)
    {
        app.views[index].invalidate_content();
    }
    match result.nav {
        Some(Nav::Page(p)) => app.views[index].go_to_page(p),
        Some(Nav::Dest(p, dest)) => match app.session.get(id) {
            Some(doc) => app.views[index].go_to_dest(p, dest, &doc.info),
            None => app.views[index].go_to_page(p),
        },
        Some(Nav::Action(target)) => {
            if let Some((source, file, destination)) = crate::canvas::follow_link(app, index, Some(target)) {
                app.open_remote_link(source, file, destination);
            }
        }
        Some(Nav::Flash(p, r)) => {
            let v = &mut app.views[index];
            if let Some(doc) = app.session.get(id) {
                v.reveal_rect(p, r, &doc.info);
            } else {
                v.go_to_page(p);
            }
            v.session.flash = Some((p, r, 0.0));
        }
        None => {}
    }
}
