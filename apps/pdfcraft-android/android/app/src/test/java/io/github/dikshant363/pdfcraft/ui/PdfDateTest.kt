package io.github.dikshant363.pdfcraft.ui

import io.github.dikshant363.pdfcraft.ui.viewer.pdfDateLabel
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Before
import org.junit.Test
import java.util.Locale
import java.util.TimeZone

class PdfDateTest {
    private val zone = TimeZone.getDefault()
    private val locale = Locale.getDefault()

    @Before
    fun setUp() {
        TimeZone.setDefault(TimeZone.getTimeZone("GMT-06:00"))
        Locale.setDefault(Locale.US)
    }

    @After
    fun tearDown() {
        TimeZone.setDefault(zone)
        Locale.setDefault(locale)
    }

    private fun label(y: Int, mo: Int, d: Int, h: Int, mi: Int): String {
        val cal = java.util.Calendar.getInstance()
        cal.clear()
        cal.set(y, mo - 1, d, h, mi)
        return java.text.DateFormat.getDateTimeInstance(java.text.DateFormat.MEDIUM, java.text.DateFormat.SHORT).format(cal.time)
    }

    @Test
    fun aCertificateDateShowsInLocalTime() {
        // the engine writes certificate validity as "2031.10.19 13:59:58 UTC"
        assertEquals(label(2031, 10, 19, 7, 59), pdfDateLabel("2031.10.19 13:59:58 UTC"))
    }

    @Test
    fun aUtcDateShowsInLocalTime() {
        // 03:28 UTC on the 12th is 21:28 on the 11th six hours west
        assertEquals(label(2026, 10, 11, 21, 28), pdfDateLabel("D:20261012032800Z"))
    }

    @Test
    fun anOffsetIsHonoured() {
        assertEquals(label(2026, 10, 11, 10, 0), pdfDateLabel("D:20261011180000+02'00'"))
        assertEquals(label(2026, 10, 11, 6, 0), pdfDateLabel("D:20261011173000+05'30'"))
    }

    @Test
    fun aDateWithoutZoneIsShownAsWritten() {
        assertEquals(label(2026, 10, 11, 17, 19), pdfDateLabel("2026-10-11 17:19"))
        assertEquals(label(2026, 10, 11, 0, 0), pdfDateLabel("D:20261011"))
        assertNull(pdfDateLabel(null))
    }
}
