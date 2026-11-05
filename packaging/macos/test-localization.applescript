-- Tests Apple's native locale resolution against the resources expanded from a PKG.
-- This only reads resources. It never starts Installer or changes language preferences.
use framework "Foundation"
use scripting additions

on run arguments
    set resourcePath to item 1 of arguments
    set fileManager to current application's NSFileManager's defaultManager()
    set names to fileManager's contentsOfDirectoryAtPath:resourcePath |error|:(missing value)
    set available to {}
    repeat with resourceName in (names as list)
        if resourceName ends with ".lproj" then set end of available to text 1 thru -7 of resourceName
    end repeat
    set cases to {{"en-AU", "en"}, {"de-AT", "de"}, {"fr-CA", "fr"}, {"es-MX", "es"}, {"nl-BE", "nl"}, {"it-CH", "it"}, {"hu-HU", "hu"}, {"ja-JP", "ja"}, {"ru-RU", "ru"}, {"pt-BR", "pt"}, {"pt-PT", "pt"}, {"zh-CN", "zh-Hans"}, {"zh-SG", "zh-Hans"}, {"zh-TW", "zh-Hant"}, {"zh-HK", "zh-Hant"}, {"zh-MO", "zh-Hant"}, {"hi-IN", "hi"}, {"bn-BD", "bn"}, {"ar-EG", "ar"}, {"id-ID", "id"}, {"ur-PK", "ur"}, {"ko-KR", "ko"}, {"tr-TR", "tr"}, {"pl-PL", "pl"}, {"sv-SE", "sv"}}
    repeat with testCase in cases
        set preferred to current application's NSBundle's preferredLocalizationsFromArray:available forPreferences:{item 1 of testCase, "de", "en"}
        set chosen to item 1 of (preferred as list)
        if chosen is not item 2 of testCase then error "Unexpected locale for " & item 1 of testCase & ": " & chosen
        set tablePath to resourcePath & "/" & chosen & ".lproj/Localizable.strings"
        set localizationTable to current application's NSDictionary's dictionaryWithContentsOfFile:tablePath
        if localizationTable is missing value then error "Unreadable localization: " & tablePath
        set descriptionKey to "Install the PdfCraft application for working with PDF documents."
        set localizedDescription to localizationTable's objectForKey:descriptionKey
        if localizedDescription is missing value then error "Missing PdfCraft choice description: " & chosen
        if chosen is in {"en", "pl", "sv"} then
            if (localizedDescription as text) is not descriptionKey then error "Non-English unsupported-language fallback: " & chosen
        else
            if (localizedDescription as text) is descriptionKey then error "Missing translated choice description: " & chosen
        end if
    end repeat
    return "ok: native locale resolution, regional variants, primary-language English fallback and PKG strings"
end run
