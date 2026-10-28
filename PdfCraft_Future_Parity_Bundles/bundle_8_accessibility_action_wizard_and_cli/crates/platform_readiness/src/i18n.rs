//! Localization Message Bundle Loader (Project Fluent Model).

use std::collections::HashMap;

pub struct FluentLocalizationCatalog {
    messages: HashMap<(String, String), String>, // (locale, key) -> translated_string
}

impl Default for FluentLocalizationCatalog {
    fn default() -> Self {
        let mut cat = Self { messages: HashMap::new() };
        cat.register_defaults();
        cat
    }
}

impl FluentLocalizationCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_message(&self, locale: &str, key: &str) -> String {
        self.messages.get(&(locale.to_string(), key.to_string()))
            .cloned()
            .or_else(|| self.messages.get(&("en".to_string(), key.to_string())).cloned())
            .unwrap_or_else(|| key.to_string())
    }

    fn register_defaults(&mut self) {
        // English
        self.messages.insert(("en".into(), "app-title".into()), "PdfCraft Professional Workstation".into());
        self.messages.insert(("en".into(), "btn-save".into()), "Save".into());
        self.messages.insert(("en".into(), "btn-cancel".into()), "Cancel".into());

        // German
        self.messages.insert(("de".into(), "app-title".into()), "PdfCraft Professionelle Workstation".into());
        self.messages.insert(("de".into(), "btn-save".into()), "Speichern".into());
        self.messages.insert(("de".into(), "btn-cancel".into()), "Abbrechen".into());

        // French
        self.messages.insert(("fr".into(), "app-title".into()), "Station de Travail Professionnelle PdfCraft".into());
        self.messages.insert(("fr".into(), "btn-save".into()), "Enregistrer".into());
        self.messages.insert(("fr".into(), "btn-cancel".into()), "Annuler".into());
    }
}
