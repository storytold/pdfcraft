//! Toggle styling for the standalone manager, which uses egui's standard visuals rather than
//! the editor theme. Keep square checkboxes distinct from circular radio buttons in both themes.

pub fn checkbox<'a>(checked: &'a mut bool, atoms: impl egui::IntoAtoms<'a>) -> impl egui::Widget + 'a {
    #[expect(clippy::disallowed_methods, reason = "standalone manager's central square-checkbox styling")]
    let widget = egui::Checkbox::new(checked, atoms);
    move |ui: &mut egui::Ui| {
        ui.scope(|ui| {
            let w = &mut ui.visuals_mut().widgets;
            for v in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
                v.corner_radius = egui::CornerRadius::same(3);
            }
            ui.add(widget)
        })
        .inner
    }
}

pub fn radio_value<'a, V: PartialEq>(ui: &mut egui::Ui, current: &mut V, alternative: V, atoms: impl egui::IntoAtoms<'a>) -> egui::Response {
    // The manager retains egui's standard outlined/filled visuals; the editor has its own theme.
    #[expect(clippy::disallowed_methods, reason = "standalone manager's central radio-button styling")]
    let widget = egui::RadioButton::new(*current == alternative, atoms);
    let mut response = ui.add(widget);
    if response.clicked() && *current != alternative {
        *current = alternative;
        response.mark_changed();
    }
    response
}
