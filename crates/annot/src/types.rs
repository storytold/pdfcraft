use super::create::normalize;
use super::*;

pub type Rgb = [f64; 3];

/// Text markup kinds (§12.5.6.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Markup {
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
}

impl Markup {
    fn subtype(self) -> &'static str {
        match self {
            Markup::Highlight => "Highlight",
            Markup::Underline => "Underline",
            Markup::StrikeOut => "StrikeOut",
            Markup::Squiggly => "Squiggly",
        }
    }
}

/// Text-annotation icon names (`/Name`). The first seven are the standard names ISO 32000-2
/// §12.5.6.4, Table 175 asks every viewer to draw; the rest are further names in common use, which
/// the same table allows ("Additional names may be supported as well").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NoteIcon {
    #[default]
    Comment,
    Note,
    Help,
    Insert,
    Key,
    NewParagraph,
    Paragraph,
    Check,
    Circle,
    Cross,
    CrossHairs,
    Star,
    RightArrow,
    UpArrow,
}

impl NoteIcon {
    pub fn name(self) -> &'static str {
        match self {
            NoteIcon::Comment => "Comment",
            NoteIcon::Note => "Note",
            NoteIcon::Help => "Help",
            NoteIcon::Insert => "Insert",
            NoteIcon::Key => "Key",
            NoteIcon::NewParagraph => "NewParagraph",
            NoteIcon::Paragraph => "Paragraph",
            NoteIcon::Check => "Check",
            NoteIcon::Circle => "Circle",
            NoteIcon::Cross => "Cross",
            NoteIcon::CrossHairs => "CrossHairs",
            NoteIcon::Star => "Star",
            NoteIcon::RightArrow => "RightArrow",
            NoteIcon::UpArrow => "UpArrow",
        }
    }

    /// Every icon, the standard names first.
    pub const ALL: [NoteIcon; 14] = [
        Self::Comment,
        Self::Note,
        Self::Help,
        Self::Insert,
        Self::Key,
        Self::NewParagraph,
        Self::Paragraph,
        Self::Check,
        Self::Circle,
        Self::Cross,
        Self::CrossHairs,
        Self::Star,
        Self::RightArrow,
        Self::UpArrow,
    ];

    pub fn from_name(n: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|i| i.name() == n)
    }
}

/// Fill & Sign marks (Acrobat's ✓, ✕, ●, ─).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillMark {
    Check,
    Cross,
    Dot,
    Line,
}

impl FillMark {
    /// The `/Name` of the stamp PdfCraft draws for it.
    pub fn name(self) -> &'static str {
        match self {
            FillMark::Check => "PCCheck",
            FillMark::Cross => "PCCross",
            FillMark::Dot => "PCDot",
            FillMark::Line => "PCLine",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FillMark::Check => "Checkmark",
            FillMark::Cross => "Cross",
            FillMark::Dot => "Dot",
            FillMark::Line => "Line",
        }
    }

    /// The user-space box of the mark placed at `at` on a page turned by `rotation`: centred
    /// there and sized in displayed axes, so a line runs across the page as displayed (#2392).
    pub fn rect_at(self, at: [f64; 2], rotation: i64) -> [f64; 4] {
        let (w, h) = if self == FillMark::Line { (36.0, 4.0) } else { (12.0, 12.0) };
        let (w, h) = if matches!(rotation.rem_euclid(360), 90 | 270) { (h, w) } else { (w, h) };
        [at[0] - w / 2.0, at[1] - h / 2.0, at[0] + w / 2.0, at[1] + h / 2.0]
    }
}

/// The stamps of Acrobat's stamp palette (drawn in PdfCraft's own style).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StampKind {
    // Standard business.
    Approved,
    Completed,
    Confidential,
    Draft,
    Final,
    ForComment,
    ForPublicRelease,
    InformationOnly,
    NotApproved,
    NotForPublicRelease,
    PreliminaryResults,
    Void,
    // Sign here.
    Accepted,
    InitialHere,
    Rejected,
    SignHere,
    Witness,
    // Dynamic (with a "By … at …" line).
    DynApproved,
    DynConfidential,
    DynReceived,
    DynReviewed,
    DynRevised,
}

/// The palette's sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StampGroup {
    Dynamic,
    SignHere,
    StandardBusiness,
}

impl StampKind {
    pub const ALL: [StampKind; 22] = [
        StampKind::DynApproved,
        StampKind::DynConfidential,
        StampKind::DynReceived,
        StampKind::DynReviewed,
        StampKind::DynRevised,
        StampKind::Accepted,
        StampKind::InitialHere,
        StampKind::Rejected,
        StampKind::SignHere,
        StampKind::Witness,
        StampKind::Approved,
        StampKind::Completed,
        StampKind::Confidential,
        StampKind::Draft,
        StampKind::Final,
        StampKind::ForComment,
        StampKind::ForPublicRelease,
        StampKind::InformationOnly,
        StampKind::NotApproved,
        StampKind::NotForPublicRelease,
        StampKind::PreliminaryResults,
        StampKind::Void,
    ];

    pub fn group(self) -> StampGroup {
        use StampKind::*;
        match self {
            DynApproved | DynConfidential | DynReceived | DynReviewed | DynRevised => StampGroup::Dynamic,
            Accepted | InitialHere | Rejected | SignHere | Witness => StampGroup::SignHere,
            _ => StampGroup::StandardBusiness,
        }
    }

    /// The text on the stamp, in English.
    pub fn label(self) -> &'static str {
        self.label_in(StampLang::En)
    }

    /// The text on the stamp in `lang`.
    pub fn label_in(self, lang: StampLang) -> &'static str {
        use StampKind::*;
        let i = match self {
            Approved | DynApproved => 0,
            Completed => 1,
            Confidential | DynConfidential => 2,
            Draft => 3,
            Final => 4,
            ForComment => 5,
            ForPublicRelease => 6,
            InformationOnly => 7,
            NotApproved => 8,
            NotForPublicRelease => 9,
            PreliminaryResults => 10,
            Void => 11,
            Accepted => 12,
            InitialHere => 13,
            Rejected => 14,
            SignHere => 15,
            Witness => 16,
            DynReceived => 17,
            DynReviewed => 18,
            DynRevised => 19,
        };
        STAMP_LABELS[lang as usize][i]
    }

    /// `/Name`: the standard stamp names of ISO 32000-2 Table 184 where one exists.
    pub fn name(self) -> &'static str {
        use StampKind::*;
        match self {
            Approved => "Approved",
            Confidential => "Confidential",
            Draft => "Draft",
            Final => "Final",
            ForComment => "ForComment",
            ForPublicRelease => "ForPublicRelease",
            NotApproved => "NotApproved",
            NotForPublicRelease => "NotForPublicRelease",
            Completed => "PCCompleted",
            InformationOnly => "PCInformationOnly",
            PreliminaryResults => "PCPreliminaryResults",
            Void => "PCVoid",
            Accepted => "PCAccepted",
            InitialHere => "PCInitialHere",
            Rejected => "PCRejected",
            SignHere => "PCSignHere",
            Witness => "PCWitness",
            DynApproved => "PCDynApproved",
            DynConfidential => "PCDynConfidential",
            DynReceived => "PCDynReceived",
            DynReviewed => "PCDynReviewed",
            DynRevised => "PCDynRevised",
        }
    }

    pub fn from_name(n: &[u8]) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name().as_bytes() == n)
    }

    /// The stamp's colour: green for approval, red for refusal and restriction, blue otherwise.
    pub fn color(self) -> Rgb {
        use StampKind::*;
        match self {
            Approved | Completed | Final | Accepted | DynApproved | DynReceived | DynReviewed => [0.13, 0.55, 0.13],
            NotApproved | Rejected | Void | Confidential | NotForPublicRelease | DynConfidential => [0.80, 0.10, 0.10],
            SignHere | InitialHere | Witness => [0.85, 0.35, 0.05],
            _ => [0.10, 0.30, 0.70],
        }
    }

    /// The stamp's size (points) for its label, as placed with a click.
    pub fn size(self) -> (f64, f64) {
        self.size_in(StampLang::En)
    }

    /// The stamp's size (points) for its label in `lang`.
    pub fn size_in(self, lang: StampLang) -> (f64, f64) {
        let w = appearance::line_width(self.label_in(lang), lang, 16.0, true) + 24.0;
        let h = if self.group() == StampGroup::Dynamic { 42.0 } else { 30.0 };
        // Room for a typical "By" line at its size (h × 0.22), so a short label (承認) doesn't
        // shrink it out of sight.
        let w = if self.group() == StampGroup::Dynamic { w.max(appearance::line_width(&lang.by_sample(), lang, h * 0.22, false) + 16.0) } else { w };
        let w = if self.group() == StampGroup::SignHere { w + 14.0 } else { w };
        (w.max(80.0), h)
    }
}

/// Stamp labels by [`StampLang`], in the order of [`StampKind::label_in`].
const STAMP_LABELS: [[&str; 20]; 36] = [
    [
        "APPROVED",
        "COMPLETED",
        "CONFIDENTIAL",
        "DRAFT",
        "FINAL",
        "FOR COMMENT",
        "FOR PUBLIC RELEASE",
        "INFORMATION ONLY",
        "NOT APPROVED",
        "NOT FOR PUBLIC RELEASE",
        "PRELIMINARY RESULTS",
        "VOID",
        "ACCEPTED",
        "INITIAL HERE",
        "REJECTED",
        "SIGN HERE",
        "WITNESS",
        "RECEIVED",
        "REVIEWED",
        "REVISED",
    ],
    [
        "APPROUVÉ",
        "TERMINÉ",
        "CONFIDENTIEL",
        "BROUILLON",
        "FINAL",
        "POUR COMMENTAIRE",
        "POUR DIFFUSION PUBLIQUE",
        "POUR INFORMATION",
        "NON APPROUVÉ",
        "DIFFUSION INTERDITE",
        "RÉSULTATS PRÉLIMINAIRES",
        "NUL",
        "ACCEPTÉ",
        "PARAPHEZ ICI",
        "REFUSÉ",
        "SIGNEZ ICI",
        "TÉMOIN",
        "REÇU",
        "REVU",
        "RÉVISÉ",
    ],
    [
        "GENEHMIGT",
        "ABGESCHLOSSEN",
        "VERTRAULICH",
        "ENTWURF",
        "FINAL",
        "ZUR STELLUNGNAHME",
        "ZUR VERÖFFENTLICHUNG",
        "NUR ZUR INFORMATION",
        "NICHT GENEHMIGT",
        "NICHT ZUR VERÖFFENTLICHUNG",
        "VORLÄUFIGE ERGEBNISSE",
        "UNGÜLTIG",
        "AKZEPTIERT",
        "HIER PARAPHIEREN",
        "ABGELEHNT",
        "HIER UNTERSCHREIBEN",
        "ZEUGE",
        "ERHALTEN",
        "GEPRÜFT",
        "ÜBERARBEITET",
    ],
    [
        "APROBADO",
        "COMPLETADO",
        "CONFIDENCIAL",
        "BORRADOR",
        "FINAL",
        "PARA COMENTARIOS",
        "PARA PUBLICACIÓN",
        "SOLO INFORMATIVO",
        "NO APROBADO",
        "NO PUBLICAR",
        "RESULTADOS PRELIMINARES",
        "NULO",
        "ACEPTADO",
        "INICIALES AQUÍ",
        "RECHAZADO",
        "FIRME AQUÍ",
        "TESTIGO",
        "RECIBIDO",
        "REVISADO",
        "MODIFICADO",
    ],
    [
        "APPROVATO",
        "COMPLETATO",
        "RISERVATO",
        "BOZZA",
        "FINALE",
        "PER COMMENTI",
        "PER PUBBLICAZIONE",
        "SOLO INFORMATIVO",
        "NON APPROVATO",
        "NON PUBBLICARE",
        "RISULTATI PRELIMINARI",
        "ANNULLATO",
        "ACCETTATO",
        "SIGLA QUI",
        "RIFIUTATO",
        "FIRMA QUI",
        "TESTIMONE",
        "RICEVUTO",
        "RIVISTO",
        "REVISIONATO",
    ],
    [
        "APROVADO",
        "CONCLUÍDO",
        "CONFIDENCIAL",
        "RASCUNHO",
        "FINAL",
        "PARA COMENTÁRIOS",
        "PARA PUBLICAÇÃO",
        "SOMENTE INFORMATIVO",
        "NÃO APROVADO",
        "NÃO PUBLICAR",
        "RESULTADOS PRELIMINARES",
        "NULO",
        "ACEITO",
        "RUBRIQUE AQUI",
        "REJEITADO",
        "ASSINE AQUI",
        "TESTEMUNHA",
        "RECEBIDO",
        "REVISADO",
        "ATUALIZADO",
    ],
    [
        "APROVAT",
        "COMPLETAT",
        "CONFIDENCIAL",
        "ESBORRANY",
        "FINAL",
        "PER A COMENTARIS",
        "PER A PUBLICACIÓ",
        "NOMÉS INFORMATIU",
        "NO APROVAT",
        "NO PUBLICAR",
        "RESULTATS PRELIMINARS",
        "NUL",
        "ACCEPTAT",
        "RUBRIQUEU AQUÍ",
        "REBUTJAT",
        "SIGNEU AQUÍ",
        "TESTIMONI",
        "REBUT",
        "REVISAT",
        "MODIFICAT",
    ],
    [
        "GODKJENT",
        "FULLFØRT",
        "KONFIDENSIELT",
        "UTKAST",
        "ENDELIG",
        "FOR KOMMENTAR",
        "FOR OFFENTLIGGJØRING",
        "KUN FOR INFORMASJON",
        "IKKE GODKJENT",
        "IKKE FOR OFFENTLIGGJØRING",
        "FORELØPIGE RESULTATER",
        "UGYLDIG",
        "AKSEPTERT",
        "PARAFER HER",
        "AVVIST",
        "SIGNER HER",
        "VITNE",
        "MOTTATT",
        "GJENNOMGÅTT",
        "REVIDERT",
    ],
    [
        "VAHVISTETTU",
        "VALMIS",
        "LUOTTAMUKSELLINEN",
        "LUONNOS",
        "LOPULLINEN",
        "KOMMENTOITAVAKSI",
        "JULKAISTAVAKSI",
        "VAIN TIEDOKSI",
        "EI HYVÄKSYTTY",
        "EI JULKAISTAVAKSI",
        "ALUSTAVAT TULOKSET",
        "MITÄTÖN",
        "HYVÄKSYTTY",
        "NIMIKIRJAIMET TÄHÄN",
        "HYLÄTTY",
        "ALLEKIRJOITA TÄHÄN",
        "TODISTAJA",
        "VASTAANOTETTU",
        "TARKASTETTU",
        "TARKISTETTU",
    ],
    [
        "DISETUJUI",
        "SELESAI",
        "RAHASIA",
        "DRAF",
        "FINAL",
        "UNTUK KOMENTAR",
        "UNTUK PUBLIKASI",
        "HANYA INFORMASI",
        "TIDAK DISETUJUI",
        "DILARANG PUBLIKASI",
        "HASIL AWAL",
        "BATAL",
        "DITERIMA",
        "PARAF DI SINI",
        "DITOLAK",
        "TANDA TANGAN DI SINI",
        "SAKSI",
        "TELAH DITERIMA",
        "DITINJAU",
        "DIREVISI",
    ],
    [
        "APROVADO",
        "CONCLUÍDO",
        "CONFIDENCIAL",
        "RASCUNHO",
        "FINAL",
        "PARA COMENTÁRIOS",
        "PARA PUBLICAÇÃO",
        "APENAS PARA INFORMAÇÃO",
        "NÃO APROVADO",
        "NÃO PUBLICAR",
        "RESULTADOS PRELIMINARES",
        "NULO",
        "ACEITE",
        "RUBRIQUE AQUI",
        "REJEITADO",
        "ASSINE AQUI",
        "TESTEMUNHA",
        "RECEBIDO",
        "REVISTO",
        "ALTERADO",
    ],
    [
        "SCHVÁLENO",
        "DOKONČENO",
        "DŮVĚRNÉ",
        "KONCEPT",
        "FINÁLNÍ",
        "KE KOMENTÁŘI",
        "KE ZVEŘEJNĚNÍ",
        "POUZE PRO INFORMACI",
        "NESCHVÁLENO",
        "NEZVEŘEJŇOVAT",
        "PŘEDBĚŽNÉ VÝSLEDKY",
        "NEPLATNÉ",
        "PŘIJATO",
        "PARAFUJTE ZDE",
        "ZAMÍTNUTO",
        "PODEPIŠTE ZDE",
        "SVĚDEK",
        "OBDRŽENO",
        "PŘEZKOUMÁNO",
        "UPRAVENO",
    ],
    [
        "ZATWIERDZONO",
        "ZAKOŃCZONO",
        "POUFNE",
        "SZKIC",
        "OSTATECZNY",
        "DO KOMENTARZA",
        "DO PUBLIKACJI",
        "TYLKO DO WIADOMOŚCI",
        "NIEZATWIERDZONO",
        "NIE DO PUBLIKACJI",
        "WYNIKI WSTĘPNE",
        "NIEWAŻNE",
        "ZAAKCEPTOWANO",
        "PARAFUJ TUTAJ",
        "ODRZUCONO",
        "PODPISZ TUTAJ",
        "ŚWIADEK",
        "OTRZYMANO",
        "PRZEJRZANO",
        "ZMIENIONO",
    ],
    [
        "ODOBRENO",
        "DOKONČANO",
        "ZAUPNO",
        "OSNUTEK",
        "KONČNO",
        "ZA KOMENTAR",
        "ZA OBJAVO",
        "SAMO ZA INFORMACIJO",
        "NEODOBRENO",
        "NE ZA OBJAVO",
        "PRELIMINARNI REZULTATI",
        "NEVELJAVNO",
        "SPREJETO",
        "PARAFIRAJTE TU",
        "ZAVRNJENO",
        "PODPIŠITE TU",
        "PRIČA",
        "PREJETO",
        "PREGLEDANO",
        "REVIDIRANO",
    ],
    [
        "ONAYLANDI",
        "TAMAMLANDI",
        "GİZLİ",
        "TASLAK",
        "NİHAİ",
        "YORUM İÇİN",
        "KAMUYA AÇIK YAYIN İÇİN",
        "YALNIZCA BİLGİ İÇİN",
        "ONAYLANMADI",
        "KAMUYA AÇIK DEĞİL",
        "ÖN SONUÇLAR",
        "GEÇERSİZ",
        "KABUL EDİLDİ",
        "BURAYA PARAF ATIN",
        "REDDEDİLDİ",
        "BURAYA İMZALAYIN",
        "TANIK",
        "ALINDI",
        "İNCELENDİ",
        "REVİZE EDİLDİ",
    ],
    [
        "PATVIRTINTA",
        "BAIGTA",
        "KONFIDENCIALU",
        "JUODRAŠTIS",
        "GALUTINIS",
        "KOMENTARAMS",
        "VIEŠINTI",
        "TIK INFORMACIJAI",
        "NEPATVIRTINTA",
        "NEVIEŠINTI",
        "PRELIMINARŪS REZULTATAI",
        "NEGALIOJA",
        "PRIIMTA",
        "PARAŠYKITE INICIALUS",
        "ATMESTA",
        "PASIRAŠYKITE ČIA",
        "LIUDININKAS",
        "GAUTA",
        "PERŽIŪRĖTA",
        "PATAISYTA",
    ],
    [
        "MIRATUAR",
        "PËRFUNDUAR",
        "KONFIDENCIAL",
        "SKICË",
        "PËRFUNDIMTAR",
        "PËR KOMENT",
        "PËR PUBLIKIM",
        "VETËM PËR INFORMIM",
        "JO I MIRATUAR",
        "MOS PUBLIKO",
        "REZULTATE PARAPRAKE",
        "ANULUAR",
        "PRANUAR",
        "PARAFONI KËTU",
        "REFUZUAR",
        "NËNSHKRUANI KËTU",
        "DËSHMITAR",
        "MARRË",
        "KONTROLLUAR",
        "RISHIKUAR",
    ],
    [
        "APROBAT",
        "FINALIZAT",
        "CONFIDENȚIAL",
        "CIORNĂ",
        "FINAL",
        "PENTRU COMENTARII",
        "PENTRU PUBLICARE",
        "DOAR PENTRU INFORMARE",
        "NEAPROBAT",
        "NU PENTRU PUBLICARE",
        "REZULTATE PRELIMINARE",
        "ANULAT",
        "ACCEPTAT",
        "PARAFAȚI AICI",
        "RESPINS",
        "SEMNAȚI AICI",
        "MARTOR",
        "PRIMIT",
        "VERIFICAT",
        "REVIZUIT",
    ],
    [
        "JÓVÁHAGYVA",
        "BEFEJEZVE",
        "BIZALMAS",
        "VÁZLAT",
        "VÉGLEGES",
        "MEGJEGYZÉSRE",
        "KÖZZÉTÉTELRE",
        "CSAK TÁJÉKOZTATÁSUL",
        "NEM JÓVÁHAGYOTT",
        "KÖZZÉTENNI TILOS",
        "ELŐZETES EREDMÉNYEK",
        "ÉRVÉNYTELEN",
        "ELFOGADVA",
        "PARAFÁLJA ITT",
        "ELUTASÍTVA",
        "ÍRJA ALÁ ITT",
        "TANÚ",
        "BEÉRKEZETT",
        "ELLENŐRIZVE",
        "ÁTDOLGOZVA",
    ],
    [
        "ОДОБРЕНО",
        "ВЫПОЛНЕНО",
        "КОНФИДЕНЦИАЛЬНО",
        "ЧЕРНОВИК",
        "ОКОНЧАТЕЛЬНО",
        "ДЛЯ ЗАМЕЧАНИЙ",
        "ДЛЯ ПУБЛИКАЦИИ",
        "ДЛЯ СВЕДЕНИЯ",
        "НЕ ОДОБРЕНО",
        "НЕ ДЛЯ ПУБЛИКАЦИИ",
        "ПРЕДВАРИТЕЛЬНЫЕ РЕЗУЛЬТАТЫ",
        "АННУЛИРОВАНО",
        "ПРИНЯТО",
        "ПОСТАВЬТЕ ИНИЦИАЛЫ",
        "ОТКЛОНЕНО",
        "ПОДПИШИТЕ ЗДЕСЬ",
        "СВИДЕТЕЛЬ",
        "ПОЛУЧЕНО",
        "ПРОВЕРЕНО",
        "ИСПРАВЛЕНО",
    ],
    [
        "ЗАТВЕРДЖЕНО",
        "ВИКОНАНО",
        "КОНФІДЕНЦІЙНО",
        "ЧЕРНЕТКА",
        "ОСТАТОЧНО",
        "ДЛЯ КОМЕНТАРІВ",
        "ДЛЯ ОПРИЛЮДНЕННЯ",
        "ДЛЯ ВІДОМА",
        "НЕ ЗАТВЕРДЖЕНО",
        "НЕ ДЛЯ ОПРИЛЮДНЕННЯ",
        "ПОПЕРЕДНІ РЕЗУЛЬТАТИ",
        "АНУЛЬОВАНО",
        "ПРИЙНЯТО",
        "ПОСТАВТЕ ІНІЦІАЛИ",
        "ВІДХИЛЕНО",
        "ПІДПИШІТЬ ТУТ",
        "СВІДОК",
        "ОТРИМАНО",
        "ПЕРЕВІРЕНО",
        "ВИПРАВЛЕНО",
    ],
    [
        "ОДОБРЕНО",
        "ЗАВЪРШЕНО",
        "ПОВЕРИТЕЛНО",
        "ЧЕРНОВА",
        "ОКОНЧАТЕЛНО",
        "ЗА КОМЕНТАР",
        "ЗА ПУБЛИКУВАНЕ",
        "ЗА СВЕДЕНИЕ",
        "НЕОДОБРЕНО",
        "НЕ ЗА ПУБЛИКУВАНЕ",
        "ПРЕДВАРИТЕЛНИ РЕЗУЛТАТИ",
        "АНУЛИРАНО",
        "ПРИЕТО",
        "ПАРАФИРАЙТЕ ТУК",
        "ОТХВЪРЛЕНО",
        "ПОДПИШЕТЕ ТУК",
        "СВИДЕТЕЛ",
        "ПОЛУЧЕНО",
        "ПРЕГЛЕДАНО",
        "РЕДАКТИРАНО",
    ],
    [
        "БЕКІТІЛДІ",
        "ОРЫНДАЛДЫ",
        "ҚҰПИЯ",
        "ЖОБА",
        "ТҮПКІЛІКТІ",
        "ПІКІР ҮШІН",
        "ЖАРИЯЛАУҒА",
        "АҚПАРАТ ҮШІН",
        "БЕКІТІЛМЕДІ",
        "ЖАРИЯЛАУҒА ЕМЕС",
        "АЛДЫН АЛА НӘТИЖЕЛЕР",
        "ЖАРАМСЫЗ",
        "ҚАБЫЛДАНДЫ",
        "ИНИЦИАЛДАРДЫ ҚОЙЫҢЫЗ",
        "ҚАБЫЛДАНБАДЫ",
        "ОСЫНДА ҚОЛ ҚОЙЫҢЫЗ",
        "КУӘГЕР",
        "АЛЫНДЫ",
        "ҚАРАЛДЫ",
        "ТҮЗЕТІЛДІ",
    ],
    [
        "ΕΓΚΡΙΘΗΚΕ",
        "ΟΛΟΚΛΗΡΩΘΗΚΕ",
        "ΕΜΠΙΣΤΕΥΤΙΚΟ",
        "ΠΡΟΣΧΕΔΙΟ",
        "ΤΕΛΙΚΟ",
        "ΓΙΑ ΣΧΟΛΙΑ",
        "ΓΙΑ ΔΗΜΟΣΙΕΥΣΗ",
        "ΜΟΝΟ ΓΙΑ ΕΝΗΜΕΡΩΣΗ",
        "ΔΕΝ ΕΓΚΡΙΘΗΚΕ",
        "ΟΧΙ ΓΙΑ ΔΗΜΟΣΙΕΥΣΗ",
        "ΠΡΟΚΑΤΑΡΚΤΙΚΑ ΑΠΟΤΕΛΕΣΜΑΤΑ",
        "ΑΚΥΡΟ",
        "ΑΠΟΔΕΚΤΟ",
        "ΜΟΝΟΓΡΑΨΤΕ ΕΔΩ",
        "ΑΠΟΡΡΙΦΘΗΚΕ",
        "ΥΠΟΓΡΑΨΤΕ ΕΔΩ",
        "ΜΑΡΤΥΡΑΣ",
        "ΠΑΡΕΛΗΦΘΗ",
        "ΕΞΕΤΑΣΤΗΚΕ",
        "ΑΝΑΘΕΩΡΗΘΗΚΕ",
    ],
    [
        "ĐÃ PHÊ DUYỆT",
        "ĐÃ HOÀN THÀNH",
        "BẢO MẬT",
        "BẢN NHÁP",
        "BẢN CUỐI",
        "ĐỂ GÓP Ý",
        "ĐỂ CÔNG BỐ",
        "CHỈ ĐỂ THAM KHẢO",
        "KHÔNG PHÊ DUYỆT",
        "KHÔNG CÔNG BỐ",
        "KẾT QUẢ SƠ BỘ",
        "VÔ HIỆU",
        "ĐÃ CHẤP NHẬN",
        "KÝ TẮT TẠI ĐÂY",
        "ĐÃ TỪ CHỐI",
        "KÝ TẠI ĐÂY",
        "NGƯỜI LÀM CHỨNG",
        "ĐÃ NHẬN",
        "ĐÃ XEM XÉT",
        "ĐÃ SỬA ĐỔI",
    ],
    [
        "承認",
        "完了",
        "機密",
        "草案",
        "最終版",
        "コメント用",
        "公開用",
        "参考資料",
        "不承認",
        "非公開",
        "暫定結果",
        "無効",
        "受理",
        "ここにイニシャル",
        "却下",
        "ここに署名",
        "立会人",
        "受領",
        "確認済み",
        "改訂済み",
    ],
    [
        "已批准",
        "已完成",
        "机密",
        "草稿",
        "最终版",
        "征求意见",
        "可公开发布",
        "仅供参考",
        "未批准",
        "不得公开发布",
        "初步结果",
        "作废",
        "已接受",
        "在此草签",
        "已拒绝",
        "在此签名",
        "见证人",
        "已收到",
        "已审阅",
        "已修订",
    ],
    [
        "已核准",
        "已完成",
        "機密",
        "草稿",
        "最終版",
        "徵求意見",
        "可公開發布",
        "僅供參考",
        "未核准",
        "不得公開發布",
        "初步結果",
        "作廢",
        "已接受",
        "在此草簽",
        "已拒絕",
        "在此簽名",
        "見證人",
        "已收到",
        "已審閱",
        "已修訂",
    ],
    [
        "승인됨",
        "완료됨",
        "기밀",
        "초안",
        "최종",
        "의견 요청",
        "공개용",
        "참고용",
        "승인 안 됨",
        "비공개",
        "예비 결과",
        "무효",
        "수락됨",
        "여기에 이니셜",
        "거부됨",
        "여기에 서명",
        "증인",
        "수신됨",
        "검토됨",
        "수정됨",
    ],
    [
        "معتمد",
        "مكتمل",
        "سري",
        "مسودة",
        "نهائي",
        "للتعليق",
        "للنشر العام",
        "للعلم فقط",
        "غير معتمد",
        "ليس للنشر العام",
        "نتائج أولية",
        "ملغى",
        "مقبول",
        "ضع الأحرف الأولى هنا",
        "مرفوض",
        "وقّع هنا",
        "شاهد",
        "مستلم",
        "تمت المراجعة",
        "منقح",
    ],
    [
        "پەسەندکرا",
        "تەواوکرا",
        "نهێنی",
        "ڕەشنووس",
        "کۆتایی",
        "بۆ ڕاوبۆچوون",
        "بۆ بڵاوکردنەوەی گشتی",
        "تەنها بۆ زانیاری",
        "پەسەندنەکرا",
        "بۆ بڵاوکردنەوە نییە",
        "ئەنجامی سەرەتایی",
        "بەتاڵ",
        "قبوڵکرا",
        "واژۆی کورت لێرە",
        "ڕەتکرایەوە",
        "لێرە واژۆ بکە",
        "شایەت",
        "وەرگیرا",
        "پێداچوونەوەی بۆ کرا",
        "دەستکاریکرا",
    ],
    [
        "อนุมัติแล้ว",
        "เสร็จสมบูรณ์",
        "ลับ",
        "ฉบับร่าง",
        "ฉบับสมบูรณ์",
        "เพื่อแสดงความคิดเห็น",
        "เพื่อเผยแพร่สู่สาธารณะ",
        "เพื่อทราบเท่านั้น",
        "ไม่อนุมัติ",
        "ห้ามเผยแพร่สู่สาธารณะ",
        "ผลเบื้องต้น",
        "ยกเลิก",
        "ยอมรับแล้ว",
        "ลงชื่อย่อที่นี่",
        "ปฏิเสธแล้ว",
        "ลงชื่อที่นี่",
        "พยาน",
        "ได้รับแล้ว",
        "ตรวจสอบแล้ว",
        "แก้ไขแล้ว",
    ],
    [
        "स्वीकृत",
        "पूर्ण",
        "गोपनीय",
        "मसौदा",
        "अंतिम",
        "टिप्पणी हेतु",
        "सार्वजनिक प्रकाशन हेतु",
        "केवल सूचनार्थ",
        "अस्वीकृत",
        "सार्वजनिक प्रकाशन हेतु नहीं",
        "प्रारंभिक परिणाम",
        "रद्द",
        "स्वीकार किया गया",
        "यहाँ आद्याक्षर करें",
        "अस्वीकार किया गया",
        "यहाँ हस्ताक्षर करें",
        "साक्षी",
        "प्राप्त",
        "समीक्षित",
        "संशोधित",
    ],
    [
        "ఆమోదించబడింది",
        "పూర్తయింది",
        "రహస్యం",
        "ముసాయిదా",
        "తుది",
        "వ్యాఖ్య కోసం",
        "బహిరంగ విడుదల కోసం",
        "సమాచారం కోసం మాత్రమే",
        "ఆమోదించబడలేదు",
        "బహిరంగ విడుదలకు కాదు",
        "ప్రాథమిక ఫలితాలు",
        "రద్దు",
        "అంగీకరించబడింది",
        "ఇక్కడ పొడి సంతకం చేయండి",
        "తిరస్కరించబడింది",
        "ఇక్కడ సంతకం చేయండి",
        "సాక్షి",
        "స్వీకరించబడింది",
        "సమీక్షించబడింది",
        "సవరించబడింది",
    ],
    [
        "ອະນຸມັດແລ້ວ",
        "ສຳເລັດແລ້ວ",
        "ລັບ",
        "ຮ່າງ",
        "ສະບັບສຸດທ້າຍ",
        "ເພື່ອສະແດງຄຳເຫັນ",
        "ເພື່ອເຜີຍແຜ່ສູ່ສາທາລະນະ",
        "ເພື່ອຊາບເທົ່ານັ້ນ",
        "ບໍ່ອະນຸມັດ",
        "ຫ້າມເຜີຍແຜ່ສູ່ສາທາລະນະ",
        "ຜົນເບື້ອງຕົ້ນ",
        "ໂມຄະ",
        "ຍອມຮັບແລ້ວ",
        "ເຊັນຫຍໍ້ບ່ອນນີ້",
        "ປະຕິເສດແລ້ວ",
        "ເຊັນບ່ອນນີ້",
        "ພະຍານ",
        "ໄດ້ຮັບແລ້ວ",
        "ກວດສອບແລ້ວ",
        "ແກ້ໄຂແລ້ວ",
    ],
    [
        "تأیید شد",
        "تکمیل شد",
        "محرمانه",
        "پیش‌نویس",
        "نهایی",
        "برای اظهارنظر",
        "برای انتشار عمومی",
        "فقط جهت اطلاع",
        "تأیید نشد",
        "انتشار عمومی ممنوع",
        "نتایج اولیه",
        "باطل",
        "پذیرفته شد",
        "اینجا پاراف کنید",
        "رد شد",
        "اینجا امضا کنید",
        "شاهد",
        "دریافت شد",
        "بررسی شد",
        "اصلاح شد",
    ],
];

/// The language a stamp's label is written in (the interface language when it is placed).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum StampLang {
    #[default]
    En,
    Fr,
    De,
    Es,
    It,
    Pt,
    Ca,
    Nb,
    Fi,
    Id,
    PtPt,
    Cs,
    Pl,
    Sl,
    Tr,
    Lt,
    Sq,
    Ro,
    Hu,
    Ru,
    Uk,
    Bg,
    Kk,
    El,
    Vi,
    Ja,
    ZhHans,
    ZhHant,
    Ko,
    Ar,
    Ckb,
    Th,
    Hi,
    Te,
    Lo,
    Fa,
}

impl StampLang {
    pub const ALL: [StampLang; 36] = [
        StampLang::En,
        StampLang::Fr,
        StampLang::De,
        StampLang::Es,
        StampLang::It,
        StampLang::Pt,
        StampLang::Ca,
        StampLang::Nb,
        StampLang::Fi,
        StampLang::Id,
        StampLang::PtPt,
        StampLang::Cs,
        StampLang::Pl,
        StampLang::Sl,
        StampLang::Tr,
        StampLang::Lt,
        StampLang::Sq,
        StampLang::Ro,
        StampLang::Hu,
        StampLang::Ru,
        StampLang::Uk,
        StampLang::Bg,
        StampLang::Kk,
        StampLang::El,
        StampLang::Vi,
        StampLang::Ja,
        StampLang::ZhHans,
        StampLang::ZhHant,
        StampLang::Ko,
        StampLang::Ar,
        StampLang::Ckb,
        StampLang::Th,
        StampLang::Hi,
        StampLang::Te,
        StampLang::Lo,
        StampLang::Fa,
    ];

    /// The language code of the UI catalog ("pt-br") or of `/PCLang`; unknown codes are English.
    pub fn from_code(code: &str) -> Self {
        Self::ALL.into_iter().find(|l| l.code() == code).unwrap_or_default()
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Fr => "fr",
            Self::De => "de",
            Self::Es => "es",
            Self::It => "it",
            Self::Pt => "pt-br",
            Self::Ca => "ca",
            Self::Nb => "nb",
            Self::Fi => "fi",
            Self::Id => "id",
            Self::PtPt => "pt-pt",
            Self::Cs => "cs",
            Self::Pl => "pl",
            Self::Sl => "sl",
            Self::Tr => "tr",
            Self::Lt => "lt",
            Self::Sq => "sq",
            Self::Ro => "ro",
            Self::Hu => "hu",
            Self::Ru => "ru",
            Self::Uk => "uk",
            Self::Bg => "bg",
            Self::Kk => "kk",
            Self::El => "el",
            Self::Vi => "vi",
            Self::Ja => "ja",
            Self::ZhHans => "zh-hans",
            Self::ZhHant => "zh-hant",
            Self::Ko => "ko",
            Self::Ar => "ar",
            Self::Ckb => "ckb",
            Self::Th => "th",
            Self::Hi => "hi",
            Self::Te => "te",
            Self::Lo => "lo",
            Self::Fa => "fa",
        }
    }

    /// The script of the language's labels (ISO 15924), which picks the faces for them.
    pub fn script(self) -> &'static str {
        match self {
            Self::En
            | Self::Fr
            | Self::De
            | Self::Es
            | Self::It
            | Self::Pt
            | Self::Ca
            | Self::Nb
            | Self::Fi
            | Self::Id
            | Self::PtPt
            | Self::Cs
            | Self::Pl
            | Self::Sl
            | Self::Tr
            | Self::Lt
            | Self::Sq
            | Self::Ro
            | Self::Hu => "Latn",
            Self::Ru => "Cyrl",
            Self::Uk => "Cyrl",
            Self::Bg => "Cyrl",
            Self::Kk => "Cyrl",
            Self::El => "Grek",
            Self::Vi => "Latn",
            Self::Ja => "Jpan",
            Self::ZhHans => "Hans",
            Self::ZhHant => "Hant",
            Self::Ko => "Kore",
            Self::Ar => "Arab",
            Self::Ckb => "Arab",
            Self::Th => "Thai",
            Self::Hi => "Deva",
            Self::Te => "Telu",
            Self::Lo => "Laoo",
            Self::Fa => "Arab",
        }
    }

    /// The language's BCP 47 tag, for shaping ("zh-Hant" picks a face's traditional forms).
    pub fn bcp47(self) -> &'static str {
        match self {
            Self::Pt => "pt-BR",
            Self::PtPt => "pt-PT",
            Self::En
            | Self::Fr
            | Self::De
            | Self::Es
            | Self::It
            | Self::Ca
            | Self::Nb
            | Self::Fi
            | Self::Id
            | Self::Cs
            | Self::Pl
            | Self::Sl
            | Self::Tr
            | Self::Lt
            | Self::Sq
            | Self::Ro
            | Self::Hu => self.code(),
            Self::Ru => "ru",
            Self::Uk => "uk",
            Self::Bg => "bg",
            Self::Kk => "kk",
            Self::El => "el",
            Self::Vi => "vi",
            Self::Ja => "ja",
            Self::ZhHans => "zh-Hans",
            Self::ZhHant => "zh-Hant",
            Self::Ko => "ko",
            Self::Ar => "ar",
            Self::Ckb => "ckb",
            Self::Th => "th",
            Self::Hi => "hi",
            Self::Te => "te",
            Self::Lo => "lo",
            Self::Fa => "fa",
        }
    }

    /// The faces a label (`bold`) or a "By" line in this language is drawn with when the
    /// standard fonts can't: the bundled Inter for Latin, Greek and Cyrillic, craft-fonts for the
    /// other scripts (see [`pdfcraft_fonts::line_faces`]).
    pub fn faces(self, bold: bool) -> Vec<&'static pdfcraft_fonts::CraftFont> {
        pdfcraft_fonts::line_faces(self.script(), bold)
    }

    /// Whether this build can draw all the language's labels and the words of its "By" line: in
    /// the standard fonts, or with [`Self::faces`]. A build without craft-fonts can't draw the
    /// scripts that need it (Japanese, Arabic, Hindi, …).
    pub fn drawable(self) -> bool {
        static DRAWABLE: std::sync::OnceLock<Vec<bool>> = std::sync::OnceLock::new();
        let all = DRAWABLE.get_or_init(|| {
            Self::ALL
                .iter()
                .map(|&lang| {
                    let words = lang.by_words().map(|(by, on, _, at, _)| format!("{by} {on} {at}")).unwrap_or_default();
                    StampKind::ALL.iter().all(|k| appearance::stamp_line_drawable(k.label_in(lang), lang, true))
                        && appearance::stamp_line_drawable(&words, lang, false)
                })
                .collect()
        });
        Self::ALL.iter().position(|l| *l == self).and_then(|i| all.get(i)).copied().unwrap_or(false)
    }

    /// The language stamps are placed in for the interface language `code`: its own when this
    /// build can draw it ([`Self::drawable`]), else English.
    pub fn for_interface(code: &str) -> Self {
        Some(Self::from_code(code)).filter(|l| l.drawable()).unwrap_or_default()
    }

    /// The dynamic stamps' sample "By …" line shown in the palette.
    pub fn by_placeholder(self) -> &'static str {
        match self {
            Self::En => "By name at time, date",
            Self::Fr => "Par nom la date et l'heure",
            Self::De => "Von Name Datum und Uhrzeit",
            Self::Es => "Por nombre fecha y hora",
            Self::It => "Da nome data e ora",
            Self::Pt => "Por nome data e hora",
            Self::Ca => "Per nom data i hora",
            Self::Nb => "Av navn dato og klokkeslett",
            Self::Fi => "Tekijä nimi päivämäärä ja aika",
            Self::Id => "Oleh nama tanggal dan waktu",
            Self::PtPt => "Por nome data e hora",
            Self::Cs => "Od jméno datum a čas",
            Self::Pl => "Przez nazwa data i godzina",
            Self::Sl => "Avtor ime datum in ura",
            Self::Tr => "Yazan: ad tarih ve saat",
            Self::Lt => "Autorius vardas data ir laikas",
            Self::Sq => "Nga emri data dhe ora",
            Self::Ro => "De nume data și ora",
            Self::Hu => "Készítette: név dátum és idő",
            Self::Ru => "Автор: имя, дата и время",
            Self::Uk => "Автор: ім'я, дата й час",
            Self::Bg => "От име на дата в час",
            Self::Kk => "Авторы: аты, күні мен уақыты",
            Self::El => "Από όνομα στις ημερομηνία ώρα",
            Self::Vi => "Bởi tên ngày giờ",
            Self::Ja => "作成者: 名前 日付 時刻",
            Self::ZhHans => "由 姓名 于 日期 时间",
            Self::ZhHant => "由 姓名 於 日期 時間",
            Self::Ko => "작성자: 이름 날짜 시간",
            Self::Ar => "بواسطة الاسم في التاريخ والوقت",
            Self::Ckb => "لەلایەن ناو لە ڕێکەوت و کات",
            Self::Th => "โดย ชื่อ วันที่ เวลา",
            Self::Hi => "द्वारा: नाम दिनांक समय",
            Self::Te => "రచయిత: పేరు తేదీ సమయం",
            Self::Lo => "ໂດຍ ຊື່ ວັນທີ ເວລາ",
            Self::Fa => "توسط نام در تاریخ و ساعت",
        }
    }

    /// A typical "By" line, as `Session::stamp_by_line_in` writes it for a short name, to size a
    /// dynamic stamp for.
    pub fn by_sample(self) -> String {
        let Some((by, on, sep, at, _)) = self.by_words() else { return "By Ada at 12:00 pm, Oct 00, 0000".into() };
        [format!("{by} Ada"), on.to_string(), format!("00{sep}00{sep}0000"), at.to_string(), "00:00".into()]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The pieces of a dynamic stamp's "By …" line: who, on, date separator, at, and whether the
    /// date runs year first. `None` for English, whose 12-hour line has its own format.
    pub fn by_words(self) -> Option<(&'static str, &'static str, char, &'static str, bool)> {
        match self {
            Self::En => None,
            Self::Fr => Some(("Par", "le", '/', "à", false)),
            Self::De => Some(("Von", "am", '.', "um", false)),
            Self::Es => Some(("Por", "el", '/', "a las", false)),
            Self::It => Some(("Da", "il", '/', "alle", false)),
            Self::Pt => Some(("Por", "em", '/', "às", false)),
            Self::Ca => Some(("Per", "el", '/', "a les", false)),
            Self::Nb => Some(("Av", "den", '.', "kl.", false)),
            Self::Fi => Some(("Tekijä", "", '.', "klo", false)),
            Self::Id => Some(("Oleh", "pada", '/', "pukul", false)),
            Self::PtPt => Some(("Por", "em", '/', "às", false)),
            Self::Cs => Some(("Od", "dne", '.', "v", false)),
            Self::Pl => Some(("Przez", "dnia", '.', "o godz.", false)),
            Self::Sl => Some(("Avtor", "dne", '.', "ob", false)),
            Self::Tr => Some(("Yazan:", "", '.', "saat", false)),
            Self::Lt => Some(("Autorius", "", '-', "", true)),
            Self::Sq => Some(("Nga", "më", '.', "në ora", false)),
            Self::Ro => Some(("De", "la", '.', "ora", false)),
            Self::Hu => Some(("Készítette:", "", '.', "", true)),
            Self::Ru => Some(("Автор:", "", '.', "", false)),
            Self::Uk => Some(("Автор:", "", '.', "", false)),
            Self::Bg => Some(("От", "на", '.', "в", false)),
            Self::Kk => Some(("Авторы:", "", '.', "", false)),
            Self::El => Some(("Από", "στις", '/', "", false)),
            Self::Vi => Some(("Bởi", "ngày", '/', "lúc", false)),
            Self::Ja => Some(("作成者:", "", '/', "", true)),
            Self::ZhHans => Some(("由", "于", '-', "", true)),
            Self::ZhHant => Some(("由", "於", '/', "", true)),
            Self::Ko => Some(("작성자:", "", '.', "", true)),
            Self::Ar => Some(("بواسطة", "في", '/', "الساعة", false)),
            Self::Ckb => Some(("لەلایەن", "لە", '/', "کاتژمێر", false)),
            Self::Th => Some(("โดย", "วันที่", '/', "เวลา", false)),
            Self::Hi => Some(("द्वारा:", "दिनांक", '/', "समय", false)),
            Self::Te => Some(("రచయిత:", "తేదీ", '/', "సమయం", false)),
            Self::Lo => Some(("ໂດຍ", "ວັນທີ", '/', "ເວລາ", false)),
            Self::Fa => Some(("توسط", "در", '/', "ساعت", false)),
        }
    }
}

/// A line ending (`/LE`): the ten styles in ISO 32000-2 Table 217.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    None,
    Square,
    Circle,
    Diamond,
    OpenArrow,
    ClosedArrow,
    Butt,
    ROpenArrow,
    RClosedArrow,
    Slash,
}

impl LineEnding {
    pub const ALL: [Self; 10] = [
        Self::None,
        Self::Square,
        Self::Circle,
        Self::Diamond,
        Self::OpenArrow,
        Self::ClosedArrow,
        Self::Butt,
        Self::ROpenArrow,
        Self::RClosedArrow,
        Self::Slash,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Square => "Square",
            Self::Circle => "Circle",
            Self::Diamond => "Diamond",
            Self::OpenArrow => "OpenArrow",
            Self::ClosedArrow => "ClosedArrow",
            Self::Butt => "Butt",
            Self::ROpenArrow => "ROpenArrow",
            Self::RClosedArrow => "RClosedArrow",
            Self::Slash => "Slash",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|ending| ending.name() == name)
    }
}

/// Geometry of a new comment, in PDF user space of its page.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// A sticky note whose icon's top-left corner is at `at`.
    Note {
        at: [f64; 2],
        icon: NoteIcon,
    },
    /// Text markup over quadrilaterals: `[x1 y1 x2 y2 x3 y3 x4 y4]` = top-left, top-right,
    /// bottom-left, bottom-right (the order Acrobat writes, §12.5.6.10).
    TextMarkup {
        kind: Markup,
        quads: Vec<[f64; 8]>,
    },
    Rectangle {
        rect: [f64; 4],
    },
    Oval {
        rect: [f64; 4],
    },
    /// A line. An open arrow at `to` is `start: None`, `end: OpenArrow` (the Arrow tool).
    Line {
        from: [f64; 2],
        to: [f64; 2],
        start: LineEnding,
        end: LineEnding,
    },
    /// Freehand strokes (Draw tool).
    Ink {
        strokes: Vec<Vec<[f64; 2]>>,
    },
    /// A text box (FreeText) showing the comment's contents.
    TextBox {
        rect: [f64; 4],
        font_size: f64,
    },
    /// Fill & Sign text typed onto the page (FreeText, typewriter intent, no border).
    Typewriter {
        rect: [f64; 4],
        font_size: f64,
    },
    /// A Fill & Sign mark in `rect`.
    Mark {
        rect: [f64; 4],
        mark: FillMark,
    },
    /// A drawn signature (ink strokes).
    Signature {
        strokes: Vec<Vec<[f64; 2]>>,
    },
    /// A rubber stamp from the stamp palette; `by` is the dynamic stamps' second line.
    Stamp {
        rect: [f64; 4],
        stamp: StampKind,
        lang: StampLang,
        by: Option<String>,
    },
    /// A typed signature or initials: filled outlines (each contour as points normalised to
    /// `rect`, 0–1, y up), drawn with the non-zero winding rule.
    TypedSignature {
        rect: [f64; 4],
        contours: Vec<Vec<[f64; 2]>>,
    },
    /// A custom stamp: a picture already in the document (an image XObject, or a form XObject
    /// whose `/Matrix` maps it to `size` points) filling `rect`, named `name`.
    CustomStamp {
        rect: [f64; 4],
        name: String,
        picture: ObjRef,
        image: bool,
        size: (f64, f64),
    },
    /// A redaction mark (§12.5.6.23) over quadrilaterals (text) or one rectangle as a quad
    /// (areas, pages). `overlay` is the text shown on the box once applied, drawn with `look`.
    Redact {
        quads: Vec<[f64; 8]>,
        overlay: String,
        look: OverlayLook,
    },
    /// A closed polygon (Polygon tool); with `cloud`, Acrobat's Cloud tool: the same polygon
    /// with a cloudy border (`/BE /S /C`, `/IT /PolygonCloud`).
    Polygon {
        vertices: Vec<[f64; 2]>,
        cloud: bool,
    },
    /// Connected lines (Polygonal Line tool). `start` is the first vertex, `end` the last.
    PolyLine {
        vertices: Vec<[f64; 2]>,
        start: LineEnding,
        end: LineEnding,
    },
    /// A text callout: a text box at `rect` with a leader line from `point` (arrowhead) via
    /// `knee` to the box (FreeText, `/IT /FreeTextCallout`, `/CL`).
    Callout {
        rect: [f64; 4],
        knee: [f64; 2],
        point: [f64; 2],
        font_size: f64,
        ending: LineEnding,
    },
    /// Insert text: a caret in `rect` (its point at the top centre).
    Caret {
        rect: [f64; 4],
    },
    /// Attach a file as a comment: an icon whose top-left corner is at `at`, holding `data`
    /// (embedded) under the name `file`.
    Attachment {
        at: [f64; 2],
        icon: AttachIcon,
        file: String,
        data: Vec<u8>,
    },
}

/// The standard file attachment icons (§12.5.6.15).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AttachIcon {
    #[default]
    PushPin,
    Paperclip,
    Graph,
    Tag,
}

impl AttachIcon {
    pub const ALL: [AttachIcon; 4] = [AttachIcon::PushPin, AttachIcon::Paperclip, AttachIcon::Graph, AttachIcon::Tag];
    pub fn name(self) -> &'static str {
        match self {
            AttachIcon::PushPin => "PushPin",
            AttachIcon::Paperclip => "Paperclip",
            AttachIcon::Graph => "Graph",
            AttachIcon::Tag => "Tag",
        }
    }
    pub fn from_name(n: &str) -> Option<AttachIcon> {
        Self::ALL.into_iter().find(|i| i.name().eq_ignore_ascii_case(n))
    }
}

/// A rectangle `[x0 y0 x1 y1]` as a quad in Acrobat's order (top-left, top-right, bottom-left,
/// bottom-right).
pub fn rect_quad(r: [f64; 4]) -> [f64; 8] {
    let [x0, y0, x1, y1] = normalize(r);
    [x0, y1, x1, y1, x0, y0, x1, y0]
}

impl Shape {
    pub fn subtype(&self) -> &'static str {
        match self {
            Shape::Note { .. } => "Text",
            Shape::TextMarkup { kind, .. } => kind.subtype(),
            Shape::Rectangle { .. } => "Square",
            Shape::Oval { .. } => "Circle",
            Shape::Line { .. } => "Line",
            Shape::Ink { .. } | Shape::Signature { .. } => "Ink",
            Shape::TextBox { .. } | Shape::Typewriter { .. } | Shape::Callout { .. } => "FreeText",
            Shape::Mark { .. } | Shape::Stamp { .. } | Shape::CustomStamp { .. } | Shape::TypedSignature { .. } => "Stamp",
            Shape::Redact { .. } => "Redact",
            Shape::Polygon { .. } => "Polygon",
            Shape::PolyLine { .. } => "PolyLine",
            Shape::Caret { .. } => "Caret",
            Shape::Attachment { .. } => "FileAttachment",
        }
    }
}

/// The standard fonts of redaction overlay text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayFont {
    #[default]
    Helvetica,
    Times,
    Courier,
}

impl OverlayFont {
    pub const ALL: [OverlayFont; 3] = [OverlayFont::Helvetica, OverlayFont::Times, OverlayFont::Courier];

    pub fn name(self) -> &'static str {
        match self {
            OverlayFont::Helvetica => "Helvetica",
            OverlayFont::Times => "Times Roman",
            OverlayFont::Courier => "Courier",
        }
    }

    /// The `/DA` font resource name (Acrobat's form font names).
    pub fn resource(self) -> &'static str {
        match self {
            OverlayFont::Helvetica => "Helv",
            OverlayFont::Times => "TiRo",
            OverlayFont::Courier => "Cour",
        }
    }

    pub fn from_resource(n: &str) -> OverlayFont {
        match n {
            "TiRo" | "Times-Roman" | "TimesRoman" => OverlayFont::Times,
            "Cour" | "Courier" => OverlayFont::Courier,
            _ => OverlayFont::Helvetica,
        }
    }
}

/// How redaction overlay text is drawn (Redaction Properties ▸ Appearance).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayLook {
    pub font: OverlayFont,
    /// Points; 0 = auto-size to fit the area.
    pub size: f64,
    pub color: Rgb,
    /// 0 left, 1 centre, 2 right.
    pub align: u8,
    /// Repeat the text to fill the area.
    pub repeat: bool,
}

impl Default for OverlayLook {
    fn default() -> Self {
        Self { font: OverlayFont::Helvetica, size: 0.0, color: [1.0, 0.0, 0.0], align: 1, repeat: false }
    }
}

/// How a comment looks.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    /// Stroke colour (text colour for text boxes, icon colour for notes).
    pub color: Rgb,
    /// 0–1 (`/CA`).
    pub opacity: f64,
    /// Border / stroke width in points (0 = none).
    pub width: f64,
    /// Interior fill for rectangles and ovals (`/IC`), background for text boxes.
    pub fill: Option<Rgb>,
}

impl Default for Style {
    fn default() -> Self {
        Self { color: [1.0, 0.82, 0.0], opacity: 1.0, width: 1.0, fill: None }
    }
}

impl Style {
    /// The default look of each commenting tool.
    pub fn default_for(shape: &Shape) -> Self {
        let (color, width) = match shape {
            Shape::Note { .. } => ([1.0, 0.82, 0.0], 1.0),
            Shape::TextMarkup { kind: Markup::Highlight, .. } => ([1.0, 0.94, 0.0], 1.0),
            Shape::TextMarkup { kind: Markup::Underline, .. } => ([0.0, 0.47, 0.84], 1.0),
            Shape::TextMarkup { kind: Markup::StrikeOut, .. } => ([0.89, 0.13, 0.13], 1.0),
            Shape::TextMarkup { kind: Markup::Squiggly, .. } => ([0.18, 0.62, 0.36], 1.0),
            Shape::Rectangle { .. } | Shape::Oval { .. } | Shape::Line { .. } | Shape::Polygon { .. } | Shape::PolyLine { .. } => {
                ([0.89, 0.13, 0.13], 2.0)
            }
            Shape::Callout { .. } => ([0.0, 0.0, 0.0], 1.0),
            Shape::Caret { .. } | Shape::Attachment { .. } => ([0.0, 0.47, 0.84], 1.0),
            Shape::Ink { .. } => ([0.0, 0.4, 0.87], 2.0),
            Shape::TextBox { .. } | Shape::Typewriter { .. } => ([0.0, 0.0, 0.0], 0.0),
            Shape::Mark { .. } => ([0.0, 0.0, 0.0], 1.5),
            Shape::Signature { .. } => ([0.0, 0.0, 0.0], 1.5),
            Shape::Stamp { stamp, .. } => (stamp.color(), 2.0),
            Shape::CustomStamp { .. } => ([0.0, 0.0, 0.0], 0.0),
            Shape::TypedSignature { .. } => ([0.0, 0.0, 0.0], 0.0),
            // Red outline while marked; a black box once applied.
            Shape::Redact { .. } => return Self { color: [0.89, 0.13, 0.13], opacity: 1.0, width: 1.0, fill: Some([0.0, 0.0, 0.0]) },
        };
        Self { color, opacity: 1.0, width, fill: None }
    }
}

/// A comment to add.
#[derive(Clone, Debug, PartialEq)]
pub struct NewAnnotation {
    /// 0-based page index.
    pub page: usize,
    pub shape: Shape,
    pub style: Style,
    /// The comment text (`/Contents`); the text shown by a text box.
    pub contents: String,
    /// `/T` (shown as the comment's author).
    pub author: String,
}

/// Values stamped onto what an edit creates or changes, supplied by the caller so edits stay
/// deterministic: a PDF date (`D:…`) and a unique id for `/NM`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Meta {
    pub date: Option<String>,
    pub id: String,
}

/// Review states a reply can set (§12.5.6.3, Table 172; Acrobat's "Set status").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewState {
    None,
    Accepted,
    Rejected,
    Cancelled,
    Completed,
}

impl ReviewState {
    pub fn name(self) -> &'static str {
        match self {
            ReviewState::None => "None",
            ReviewState::Accepted => "Accepted",
            ReviewState::Rejected => "Rejected",
            ReviewState::Cancelled => "Cancelled",
            ReviewState::Completed => "Completed",
        }
    }

    pub fn from_name(n: &str) -> Option<Self> {
        [Self::None, Self::Accepted, Self::Rejected, Self::Cancelled, Self::Completed].into_iter().find(|s| s.name().eq_ignore_ascii_case(n))
    }
}

// ── building ────────────────────────────────────────────────────────────────────────────────

/// Annotation flags (§12.5.3).
pub(crate) const FLAG_PRINT: i64 = 4;

pub(crate) const FLAG_NO_ZOOM: i64 = 8;

pub(crate) const FLAG_NO_ROTATE: i64 = 16;

pub(crate) const FLAG_HIDDEN: i64 = 2;

pub(crate) const FLAG_NO_VIEW: i64 = 32;

pub(crate) const FLAG_LOCKED: i64 = 128;

/// Size of a note icon (points, unscaled by zoom).
pub const NOTE_SIZE: f64 = 20.0;
