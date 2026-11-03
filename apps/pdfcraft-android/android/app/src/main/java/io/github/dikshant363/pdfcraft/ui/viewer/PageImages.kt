package io.github.dikshant363.pdfcraft.ui.viewer

import android.util.Log
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import io.github.dikshant363.pdfcraft.core.TileRect
import io.github.dikshant363.pdfcraft.engine.Engine
import io.github.dikshant363.pdfcraft.engine.toBitmap
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * What to draw: a whole page at the zoom-1 scale ([tile] null), or one tile of it at a deeper
 * zoom level. [gen] is the document's edit generation, so an edit asks for new pixels.
 */
data class ImageKey(val page: Int, val gen: Long, val level: Int, val tile: Int = -1) {
    val isBase get() = tile < 0
}

/** A rendered image and where it goes: a page, or a tile's rectangle in device pixels at its scale. */
class PageImage(val key: ImageKey, val bitmap: ImageBitmap, val scale: Float, val x: Int, val y: Int) {
    val bytes: Int get() = bitmap.width * bitmap.height * 4
}

/** A request: the key, and how to render it (page, scale, tile). */
data class ImageRequest(val key: ImageKey, val scale: Float, val tile: TileRect?)

/**
 * The rendered pages and tiles of one document, and the workers that render what the view asks
 * for, most wanted first. The view calls [want] with every frame's list; whatever is no longer
 * wanted when a worker gets to it is skipped. Images are kept up to [budgetBytes] (least
 * recently used go first), and a page's latest whole-page image is kept as a stand-in while its
 * pixels for a newer generation render.
 */
class PageImages(
    private val engine: Engine,
    private val doc: ULong,
    private val scope: CoroutineScope,
    private val budgetBytes: Int,
    workers: Int = 2,
) {
    private val lock = Any()
    private val cache = LinkedHashMap<ImageKey, PageImage>(64, 0.75f, true)
    private val fallback = HashMap<Int, PageImage>()
    private var wanted: List<ImageRequest> = emptyList()
    private val inFlight = HashSet<ImageKey>()
    private val failed = HashSet<ImageKey>()
    private var bytes = 0
    private val signal = Channel<Unit>(Channel.CONFLATED)

    /** Bumped whenever an image arrives: the canvas reads it to redraw. */
    var version by mutableIntStateOf(0)
        private set

    /** The workers' job: cancelled when the document closes. */
    private val job = SupervisorJob(scope.coroutineContext[Job])

    init {
        repeat(workers) { scope.launch(job) { work() } }
    }

    /** Stop the workers and drop the images (the document closed). */
    fun close() {
        job.cancel()
        clear()
    }

    /** The images the view wants now, most wanted first. */
    fun want(requests: List<ImageRequest>) {
        synchronized(lock) { wanted = requests }
        signal.trySend(Unit)
    }

    fun get(key: ImageKey): PageImage? = synchronized(lock) { cache[key] }

    /** The newest whole-page image of [page], whatever its generation. */
    fun latestBase(page: Int): PageImage? = synchronized(lock) { fallback[page] }

    /** Forget everything (the document was replaced). */
    fun clear() = synchronized(lock) {
        cache.clear()
        fallback.clear()
        failed.clear()
        bytes = 0
    }

    /** Drop everything but what is on screen (the system is short of memory). */
    fun trim(keep: Set<ImageKey>) = synchronized(lock) {
        val it = cache.entries.iterator()
        while (it.hasNext()) {
            val e = it.next()
            if (e.key !in keep) {
                bytes -= e.value.bytes
                it.remove()
            }
        }
        fallback.values.retainAll { it.key in keep }
    }

    private fun next(): ImageRequest? = synchronized(lock) {
        val r = wanted.firstOrNull { it.key !in cache && it.key !in inFlight && it.key !in failed } ?: return null
        inFlight.add(r.key)
        r
    }

    private suspend fun work() {
        while (true) {
            val r = next()
            if (r == null) {
                signal.receive()
                continue
            }
            val image = try {
                withContext(engine.renders) {
                    val img = engine.session.renderPage(doc, r.key.page.toUInt(), r.scale, r.tile)
                    PageImage(r.key, img.toBitmap().asImageBitmap(), r.scale, r.tile?.x?.toInt() ?: 0, r.tile?.y?.toInt() ?: 0)
                }
            } catch (e: Exception) {
                Log.w("PdfCraft", "render of ${r.key} failed: ${e.message}")
                synchronized(lock) { failed.add(r.key) }
                null
            } finally {
                synchronized(lock) { inFlight.remove(r.key) }
            }
            if (image != null) {
                synchronized(lock) {
                    cache.put(r.key, image)?.let { bytes -= it.bytes }
                    bytes += image.bytes
                    if (r.key.isBase) {
                        val old = fallback[r.key.page]
                        if (old == null || old.key.gen <= r.key.gen) fallback[r.key.page] = image
                    }
                    evict()
                }
                version++
            }
            // let the other workers and the next frame's wishes in
            signal.trySend(Unit)
        }
    }

    /** Drop least recently used images over budget. */
    private fun evict() {
        if (bytes <= budgetBytes) return
        // First pass: keep active requests (highest priority on-screen requests), evicting off-screen prefetches
        val keep = wanted.take(8).mapTo(HashSet()) { it.key }
        val it = cache.entries.iterator()
        while (bytes > budgetBytes && it.hasNext()) {
            val e = it.next()
            if (e.key in keep) continue
            bytes -= e.value.bytes
            it.remove()
            if (fallback[e.key.page]?.key == e.key) fallback.remove(e.key.page)
        }
        // Second pass: if still over budget, evict least recently used entries to enforce memory bound
        val it2 = cache.entries.iterator()
        while (bytes > budgetBytes && it2.hasNext()) {
            val e = it2.next()
            bytes -= e.value.bytes
            it2.remove()
            if (fallback[e.key.page]?.key == e.key) fallback.remove(e.key.page)
        }
    }
}
