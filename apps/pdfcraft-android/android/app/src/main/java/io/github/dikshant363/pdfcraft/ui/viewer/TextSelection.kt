package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import io.github.dikshant363.pdfcraft.core.TextRange
import io.github.dikshant363.pdfcraft.engine.Engine
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** The selected text on one page, its rectangles (points), and the glyphs its two handles sit on. */
data class Selection(val range: TextRange, val rects: List<FloatArray>, val startRect: FloatArray?, val endRect: FloatArray?)

/**
 * Text selection: a long press selects the word under the finger, the handles at either end
 * extend it glyph by glyph, and a mouse drag selects the text it crosses. A selection stays on one page, as a phone screen shows one page's
 * text at a time.
 */
@Stable
class TextSelectionState(private val engine: Engine, private val doc: ULong, private val scope: CoroutineScope) {
    var selection by mutableStateOf<Selection?>(null)
        private set

    /** Where a handle is being dragged (screen), for the magnifier over it; null when none is. */
    var magnifierAt by mutableStateOf<androidx.compose.ui.geometry.Offset?>(null)

    /** The glyph the selection started from, which stays put while the other handle moves. */
    private var anchor = 0

    /** Select the word at [p]; false when there is no text there. */
    suspend fun selectWordAt(p: PagePoint, slopPoints: Float): Boolean {
        val range = withContext(engine.commands) { engine.session.wordAt(doc, p.page.toUInt(), p.x, p.y, slopPoints) } ?: return false
        anchor = range.start.toInt()
        show(range)
        return true
    }

    // the latest mouse drag waiting to be shown, and the work showing one (one at a time)
    private var span: Pair<PagePoint, PagePoint>? = null
    private var spanJob: Job? = null

    /**
     * A mouse drag from [from] to [to] on one page: the text between them, as a desktop selects.
     * Moves arrive faster than the engine answers, so only the latest is shown.
     */
    fun selectSpan(from: PagePoint, to: PagePoint) {
        span = from to to
        if (spanJob?.isActive == true) return
        spanJob = scope.launch {
            while (true) {
                val (a, b) = span ?: break
                span = null
                val page = a.page.toUInt()
                val (ga, gb) = withContext(engine.commands) {
                    engine.session.nearestGlyph(doc, page, a.x, a.y) to engine.session.nearestGlyph(doc, page, b.x, b.y)
                }
                if (ga == null || gb == null) continue
                anchor = ga.toInt()
                show(TextRange(page, minOf(ga, gb), maxOf(ga, gb) + 1u))
            }
        }
    }

    /** Select all the text of [page]. */
    fun selectPage(page: Int) {
        scope.launch {
            val n = withContext(engine.commands) { engine.session.glyphCount(doc, page.toUInt()) }
            if (n == 0u) return@launch
            anchor = 0
            show(TextRange(page.toUInt(), 0u, n))
        }
    }

    /** A handle moved to [p] on the selection's page: [start] says which one. */
    fun dragHandle(start: Boolean, p: PagePoint) {
        val sel = selection ?: return
        if (p.page.toUInt() != sel.range.page) return
        scope.launch {
            val g = withContext(engine.commands) { engine.session.nearestGlyph(doc, sel.range.page, p.x, p.y) } ?: return@launch
            val fixed = if (start) sel.range.end.toInt() - 1 else sel.range.start.toInt()
            val a = minOf(fixed, g.toInt())
            val b = maxOf(fixed, g.toInt()) + 1
            anchor = fixed
            show(TextRange(sel.range.page, a.toUInt(), b.toUInt()))
        }
    }

    private suspend fun show(range: TextRange) {
        val (rects, s, e) = withContext(engine.commands) {
            Triple(
                engine.session.textRects(doc, range),
                engine.session.glyphRect(doc, range.page, range.start),
                engine.session.glyphRect(doc, range.page, if (range.end > range.start) range.end - 1u else range.start),
            )
        }
        selection = Selection(range, rects.toList().chunked(4).filter { it.size == 4 }.map { it.toFloatArray() },
            s.takeIf { it.size == 4 }?.toFloatArray(), e.takeIf { it.size == 4 }?.toFloatArray())
    }

    suspend fun text(): String? {
        val sel = selection ?: return null
        return withContext(engine.commands) { engine.session.textOf(doc, sel.range) }
    }

    fun clear() {
        selection = null
    }
}
