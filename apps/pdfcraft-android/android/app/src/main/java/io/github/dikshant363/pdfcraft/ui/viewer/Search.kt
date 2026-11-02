package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import io.github.dikshant363.pdfcraft.core.SearchHit
import io.github.dikshant363.pdfcraft.engine.Engine
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Find in the document: the hits of the current query, found page by page from the page the
 * reader is on and wrapping round, so the first hit shows quickly in a long document. The hits
 * are kept in page order whatever order they were found in.
 */
@Stable
class SearchState(private val engine: Engine, private val doc: ULong, private val scope: CoroutineScope) {
    var query by mutableStateOf("")
        private set
    var caseSensitive by mutableStateOf(false)
    var wholeWords by mutableStateOf(false)

    /** Hits in reading order. */
    val hits = mutableStateListOf<SearchHit>()

    /** The hit the view shows (an index into [hits]), or -1. */
    var current by mutableIntStateOf(-1)
        private set

    /** Pages searched so far, and whether the search is still going. */
    var searched by mutableIntStateOf(0)
        private set
    var running by mutableStateOf(false)
        private set

    private var job: Job? = null

    /**
     * Search for [text] over [pageCount] pages starting at [fromPage]; [onFirst] is called with
     * the first hit found (so the view can go there).
     */
    fun run(text: String, pageCount: Int, fromPage: Int, onFirst: (SearchHit) -> Unit) {
        job?.cancel()
        query = text
        hits.clear()
        current = -1
        searched = 0
        if (text.isBlank() || pageCount <= 0) {
            running = false
            return
        }
        running = true
        val case = caseSensitive
        val whole = wholeWords
        job = scope.launch {
            var announced = false
            for (k in 0 until pageCount) {
                if (!isActive) return@launch
                val page = (fromPage + k) % pageCount
                val found = runCatching {
                    withContext(engine.thumbnails) { engine.session.findInPage(doc, page.toUInt(), text, case, whole) }
                }.getOrDefault(emptyList())
                if (found.isNotEmpty()) {
                    // keep reading order: insert this page's hits where they belong
                    val at = hits.indexOfFirst { it.range.page.toInt() > page }.let { if (it < 0) hits.size else it }
                    hits.addAll(at, found)
                    if (current >= at) current += found.size
                    if (!announced) {
                        announced = true
                        current = at
                        onFirst(found.first())
                    }
                }
                searched = k + 1
            }
            running = false
        }
    }

    /** Step to the next (or previous) hit, wrapping round; the hit to show. */
    fun step(forward: Boolean): SearchHit? {
        if (hits.isEmpty()) return null
        current = if (current < 0) 0 else (current + if (forward) 1 else -1).mod(hits.size)
        return hits[current]
    }

    fun select(index: Int): SearchHit? {
        if (index !in hits.indices) return null
        current = index
        return hits[index]
    }

    fun clear() {
        job?.cancel()
        query = ""
        hits.clear()
        current = -1
        searched = 0
        running = false
    }

    val currentHit: SearchHit? get() = hits.getOrNull(current)
}
