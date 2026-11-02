package io.github.dikshant363.pdfcraft.ui

import io.github.dikshant363.pdfcraft.ui.viewer.fitSignatureBox
import io.github.dikshant363.pdfcraft.ui.viewer.readableName
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test

class SignatureBoxTest {
    @Test
    fun aDraggedBoxBigEnoughIsKept() {
        val r = floatArrayOf(100f, 200f, 300f, 260f)
        assertArrayEquals(r, fitSignatureBox(r, 0f, 0f, 612f, 792f), 0.001f)
    }

    @Test
    fun aTapGivesTheUsualBoxCentredThere() {
        assertArrayEquals(floatArrayOf(210f, 373f, 390f, 427f), fitSignatureBox(null, 300f, 400f, 612f, 792f), 0.001f)
    }

    @Test
    fun aSmallBoxGrowsAroundItsCentre() {
        assertArrayEquals(floatArrayOf(210f, 373f, 390f, 427f), fitSignatureBox(floatArrayOf(290f, 395f, 310f, 405f), 0f, 0f, 612f, 792f), 0.001f)
    }

    @Test
    fun theBoxStaysOnThePage() {
        // tapped in the bottom-right corner: pushed back inside
        assertArrayEquals(floatArrayOf(432f, 738f, 612f, 792f), fitSignatureBox(null, 610f, 790f, 612f, 792f), 0.001f)
        // a page smaller than the usual box: the box is the page
        assertArrayEquals(floatArrayOf(0f, 0f, 100f, 40f), fitSignatureBox(null, 50f, 20f, 100f, 40f), 0.001f)
    }

    @Test
    fun aCertificateNameReadsAsItsCommonName() {
        assertEquals("PrintCraft Test Root CA", readableName("CN=PrintCraft Test Root CA, O=PrintCraft Tests"))
        assertEquals("PrintCraft Tests", readableName("O=PrintCraft Tests, C=US"))
        assertEquals("Somebody", readableName("C=US, CN=Somebody"))
        assertEquals("unparsed", readableName("unparsed"))
    }
}
