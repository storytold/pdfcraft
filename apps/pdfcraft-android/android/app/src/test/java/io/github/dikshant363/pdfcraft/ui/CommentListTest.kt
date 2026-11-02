package io.github.dikshant363.pdfcraft.ui

import io.github.dikshant363.pdfcraft.ui.viewer.commentDateLabel
import io.github.dikshant363.pdfcraft.ui.viewer.modifiedLabel
import io.github.dikshant363.pdfcraft.ui.viewer.threadsTitle
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Before
import org.junit.Test
import java.util.Calendar
import java.util.Locale
import java.util.TimeZone

class CommentListTest {
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

    private fun at(y: Int, mo: Int, d: Int, h: Int, mi: Int): Long = Calendar.getInstance().apply {
        clear()
        set(y, mo - 1, d, h, mi)
    }.timeInMillis

    // the app passes in its translated words; the tests use the English ones
    private fun dateLabel(raw: String?, now: Long) = commentDateLabel(raw, "Today", "Yesterday", now)

    private fun threads(n: Int) = threadsTitle(n, "No comments") { if (it == 1) "1 comment thread" else "$it comment threads" }

    @Test
    fun commentDatesSayTodayYesterdayOrTheDate() {
        val now = at(2026, 10, 12, 9, 0)
        assertEquals("Today", dateLabel("D:20261012080000-06'00'", now))
        assertEquals("Yesterday", dateLabel("D:20261011235900-06'00'", now))
        assertEquals("Oct 10, 2026", dateLabel("D:20261010120000-06'00'", now))
        // 03:00 UTC on the 12th is still the 11th here
        assertEquals("Yesterday", dateLabel("D:20261012030000Z", now))
        assertNull(dateLabel(null, now))
    }

    @Test
    fun threadCountsReadNaturally() {
        assertEquals("No comments", threads(0))
        assertEquals("1 comment thread", threads(1))
        assertEquals("17 comment threads", threads(17))
    }

    @Test
    fun theFileDateIsTheTimeTodayThenYesterdayThenTheDate() {
        val now = at(2026, 10, 12, 9, 0)
        assertEquals("Yesterday", modifiedLabel(at(2026, 10, 11, 21, 39), "Yesterday", now))
        assertEquals("Oct 9, 2026", modifiedLabel(at(2026, 10, 9, 10, 0), "Yesterday", now))
        assertEquals(java.text.DateFormat.getTimeInstance(java.text.DateFormat.SHORT).format(java.util.Date(at(2026, 10, 12, 7, 58))), modifiedLabel(at(2026, 10, 12, 7, 58), "Yesterday", now))
    }
}
