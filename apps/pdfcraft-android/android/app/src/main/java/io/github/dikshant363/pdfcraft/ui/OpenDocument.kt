package io.github.dikshant363.pdfcraft.ui

import android.net.Uri
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import io.github.dikshant363.pdfcraft.core.DocumentInfo
import io.github.dikshant363.pdfcraft.engine.Engine
import io.github.dikshant363.pdfcraft.ui.viewer.LinkFlash
import io.github.dikshant363.pdfcraft.ui.viewer.PageImages
import io.github.dikshant363.pdfcraft.ui.viewer.PageSize
import io.github.dikshant363.pdfcraft.ui.viewer.SearchState
import io.github.dikshant363.pdfcraft.ui.viewer.TextSelectionState
import io.github.dikshant363.pdfcraft.ui.viewer.ViewerController
import io.github.dikshant363.pdfcraft.ui.viewer.ViewerState
import java.io.File
import kotlinx.coroutines.CoroutineScope

/**
 * A document open in the viewer: the engine's id for it, what the engine says about it, where it
 * came from (so saving can write back), and its view, tools and search.
 */
@Stable
class OpenDocument(
    val id: ULong,
    info: DocumentInfo,
    /** Where it was opened from (a picked or shared document, or a file), for writing back. */
    val source: Uri?,
    /** The working copy the engine reads and saves. */
    val file: File,
    val images: PageImages,
    /** The password it was opened with. */
    val password: String?,
    engine: Engine,
    scope: CoroutineScope,
) {
    var info by mutableStateOf(info)
        private set

    val viewer = ViewerState(info.pages.map { PageSize(it.width, it.height) })
    val controller = ViewerController()
    val search = SearchState(engine, id, scope)
    val selection = TextSelectionState(engine, id, scope)

    /** A link's area flashing after it was tapped. */
    var linkFlash by mutableStateOf<LinkFlash?>(null)

    /** Whether saving can write back to [source] (else changes need Save a copy). */
    var writable by mutableStateOf(true)

    /** Changes saved into [file] that [source] doesn't have (it couldn't be written): closing offers a copy. */
    var notWrittenBack = false

    /** The links of each page (0-based), refreshed when the document changes. */
    var links: Map<Int, List<io.github.dikshant363.pdfcraft.core.LinkInfo>> = emptyMap()

    /** The comments, refreshed when the document changes (tapping one selects it). */
    var comments: List<io.github.dikshant363.pdfcraft.core.CommentInfo> = emptyList()

    /** Its signatures and signature fields with what validating them found, refreshed when the document changes. */
    var signatures by mutableStateOf<List<io.github.dikshant363.pdfcraft.ui.viewer.SignatureEntry>>(emptyList())

    /** The form fields' widgets, refreshed when the document changes. */
    var widgets: List<io.github.dikshant363.pdfcraft.core.WidgetInfo> = emptyList()

    /** What the active tool still holds (ink being drawn), to add when the tool is left. */
    var commitTool: (() -> Unit)? = null

    /**
     * Undo and redo inside the active tool (strokes drawn but not yet added): while it is set the
     * top bar's undo and redo act on it and never reach the document, as the reference's do.
     */
    var localUndo by mutableStateOf<LocalUndo?>(null)

    /** The file name without its extension, as the reference titles documents. */
    val title: String get() = info.name.removeSuffix(".pdf").removeSuffix(".PDF")
    val name: String get() = info.name
    val pageCount: Int get() = info.pages.size

    /** The engine's view of the document changed (an edit, undo, save): refresh what follows from it. */
    fun update(newInfo: DocumentInfo) {
        val pagesChanged = newInfo.pages.size != info.pages.size ||
            newInfo.pages.zip(info.pages).any { (a, b) -> a.width != b.width || a.height != b.height }
        info = newInfo
        if (pagesChanged) viewer.pages = newInfo.pages.map { PageSize(it.width, it.height) }
        selection.clear()
    }
}

/** A tool's own undo history (see [OpenDocument.localUndo]); the checks read Compose state. */
class LocalUndo(val canUndo: () -> Boolean, val canRedo: () -> Boolean, val undo: () -> Unit, val redo: () -> Unit)
