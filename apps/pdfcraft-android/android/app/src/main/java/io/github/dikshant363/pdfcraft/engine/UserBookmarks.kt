package io.github.dikshant363.pdfcraft.engine

import androidx.compose.runtime.mutableStateMapOf
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.text
import java.io.File
import org.json.JSONArray
import org.json.JSONObject

/** A bookmark the reader added: its name and page (0-based). */
data class UserBookmark(val name: String, val page: Int)

/**
 * The reader's own bookmarks, per document (keyed by where it was opened from), kept on the
 * device as the reference keeps them, not in the PDF. Reads come from memory; writes go to a
 * small JSON file.
 */
class UserBookmarks(private val file: File) {
    private val lock = Any()
    private val byDoc = mutableStateMapOf<String, List<UserBookmark>>()

    init {
        runCatching {
            if (file.isFile) {
                val o = JSONObject(file.readText())
                for (key in o.keys()) {
                    val a = o.getJSONArray(key)
                    byDoc[key] = (0 until a.length()).mapNotNull { i ->
                        a.optJSONObject(i)?.let { UserBookmark(it.optString("name", text(R.string.files_bookmark_page, it.optInt("page") + 1)), it.optInt("page")) }
                    }
                }
            }
        }
    }

    fun list(key: String): List<UserBookmark> = byDoc[key].orEmpty().sortedBy { it.page }

    fun has(key: String, page: Int) = byDoc[key].orEmpty().any { it.page == page }

    /** Add a bookmark on [page] named "Page N", or remove the one there; returns the bookmark and whether it was added. */
    fun toggle(key: String, page: Int): Pair<UserBookmark, Boolean> = synchronized(lock) {
        val list = byDoc[key].orEmpty()
        val existing = list.firstOrNull { it.page == page }
        if (existing != null) {
            byDoc[key] = list - existing
            save()
            existing to false
        } else {
            val b = UserBookmark(text(R.string.files_bookmark_page, page + 1), page)
            byDoc[key] = list + b
            save()
            b to true
        }
    }

    fun rename(key: String, b: UserBookmark, name: String) = synchronized(lock) {
        byDoc[key] = byDoc[key].orEmpty().map { if (it == b) it.copy(name = name) else it }
        save()
    }

    fun remove(key: String, b: UserBookmark) = synchronized(lock) {
        byDoc[key] = byDoc[key].orEmpty() - b
        save()
    }

    /** The file was renamed: its bookmarks go with it (as the reference moves them). */
    fun move(from: String, to: String) = synchronized(lock) {
        if (from == to) return@synchronized
        val list = byDoc.remove(from) ?: return@synchronized
        byDoc[to] = list
        save()
    }

    fun clear(key: String) = synchronized(lock) {
        byDoc.remove(key)
        save()
    }

    private fun save() {
        val o = JSONObject()
        for ((k, list) in byDoc) {
            if (list.isEmpty()) continue
            o.put(k, JSONArray().apply { list.forEach { put(JSONObject().put("name", it.name).put("page", it.page)) } })
        }
        runCatching {
            val tmp = File(file.parentFile, file.name + ".tmp")
            tmp.writeText(o.toString())
            if (!tmp.renameTo(file)) {
                tmp.copyTo(file, overwrite = true)
                tmp.delete()
            }
        }.onFailure {
            android.util.Log.w("PdfCraft", "failed to save bookmarks: ${it.message}")
        }
    }
}
