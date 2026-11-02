package io.github.dikshant363.pdfcraft.ui

import io.github.dikshant363.pdfcraft.ui.viewer.ReadAloud
import org.junit.Assert.assertEquals
import org.junit.Test

class ReadAloudWordsTest {
    @Test
    fun wordsAreRunsOfLettersAndDigits() {
        assertEquals(0, ReadAloud.countWords(""))
        assertEquals(4, ReadAloud.countWords("A PDF reader, judged!"))
        assertEquals(3, ReadAloud.countWords("page 12 — done"))
    }

    @Test
    fun eachCjkCharacterIsAWord() {
        assertEquals(3, ReadAloud.countWords("縦書き"))
        assertEquals(6, ReadAloud.countWords("Hi 世界 and 한국"))
    }
}
