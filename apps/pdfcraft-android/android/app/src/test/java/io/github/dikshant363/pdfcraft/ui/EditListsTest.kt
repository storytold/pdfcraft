package io.github.dikshant363.pdfcraft.ui

import io.github.dikshant363.pdfcraft.ui.viewer.ListStyle
import org.junit.Assert.assertEquals
import org.junit.Test

class EditListsTest {
    private val items = "Apples\nPears\nPlums"

    @Test
    fun eachLineGetsItsMarker() {
        assertEquals("• Apples\n• Pears\n• Plums", ListStyle.apply(items, ListStyle.Bullet))
        assertEquals("1. Apples\n2. Pears\n3. Plums", ListStyle.apply(items, ListStyle.Number))
        assertEquals("i. Apples\nii. Pears\niii. Plums", ListStyle.apply(items, ListStyle.Roman))
        assertEquals("A. Apples\nB. Pears\nC. Plums", ListStyle.apply(items, ListStyle.Upper))
    }

    @Test
    fun aListIsRecognisedAndCanBeChangedOrRemoved() {
        val numbered = ListStyle.apply(items, ListStyle.Number)
        assertEquals(ListStyle.Number, ListStyle.of(numbered))
        assertEquals("a. Apples\nb. Pears\nc. Plums", ListStyle.apply(numbered, ListStyle.Lower))
        assertEquals(items, ListStyle.apply(numbered, ListStyle.None))
        assertEquals(ListStyle.Roman, ListStyle.of(ListStyle.apply(items, ListStyle.Roman)))
    }

    @Test
    fun plainTextIsNotTakenForAList() {
        assertEquals(ListStyle.None, ListStyle.of("a. b. c. are letters\nnot a list"))
        assertEquals(ListStyle.None, ListStyle.of(items))
        assertEquals(items, ListStyle.apply(items, ListStyle.None))
    }

    @Test
    fun blankLinesKeepTheirPlaceAndAreNotCounted() {
        assertEquals("1. One\n\n2. Two", ListStyle.apply("One\n\nTwo", ListStyle.Number))
    }
}
