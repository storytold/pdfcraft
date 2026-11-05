use framework "Foundation"
set languageCodes to current application's NSLocale's ISOLanguageCodes() as list
set AppleScript's text item delimiters to linefeed
return languageCodes as text
