package io.github.dikshant363.pdfcraft.ui

import io.github.dikshant363.pdfcraft.ui.viewer.languageIndex
import java.util.Locale
import org.junit.Assert.assertEquals
import org.junit.Test

class ReadAloudLanguageTest {
    private val langs = listOf(Locale("deu", "DEU"), Locale("eng", "GBR"), Locale("eng", "USA"), Locale("fra", "FRA"))

    @Test
    fun twoLetterLocaleFindsItsThreeLetterTwin() {
        assertEquals(2, languageIndex(langs, Locale.US))
    }

    @Test
    fun sameLanguageWhenTheCountryIsMissing() {
        assertEquals(1, languageIndex(langs, Locale("en", "AU")))
    }

    @Test
    fun missingLanguage() {
        assertEquals(-1, languageIndex(langs, Locale.JAPAN))
    }
}
