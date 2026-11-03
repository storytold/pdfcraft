package io.github.dikshant363.pdfcraft.ui

import android.app.ActivityManager
import io.github.dikshant363.pdfcraft.ui.home.FileTool
import io.github.dikshant363.pdfcraft.ui.viewer.FillElement
import androidx.compose.ui.graphics.asImageBitmap
import io.github.dikshant363.pdfcraft.engine.toBitmap
import io.github.dikshant363.pdfcraft.engine.UserBookmark
import io.github.dikshant363.pdfcraft.engine.UserBookmarks
import io.github.dikshant363.pdfcraft.ui.viewer.NotePrompt
import io.github.dikshant363.pdfcraft.ui.viewer.VoicePrompt
import io.github.dikshant363.pdfcraft.ui.viewer.ReplyPrompt
import io.github.dikshant363.pdfcraft.ui.viewer.StyledTool
import io.github.dikshant363.pdfcraft.ui.viewer.TextMarkup
import io.github.dikshant363.pdfcraft.ui.viewer.TextPrompt
import io.github.dikshant363.pdfcraft.ui.viewer.ToolPrefs
import io.github.dikshant363.pdfcraft.ui.viewer.engineType
import io.github.dikshant363.pdfcraft.ui.viewer.hex
import android.content.ClipData
import android.content.Intent
import androidx.compose.runtime.snapshotFlow
import io.github.dikshant363.pdfcraft.ui.viewer.LinkFlash
import io.github.dikshant363.pdfcraft.ui.viewer.PagePoint
import io.github.dikshant363.pdfcraft.ui.viewer.ReadAloud
import io.github.dikshant363.pdfcraft.ui.viewer.ViewerTool
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import org.json.JSONObject
import android.app.Application
import android.net.Uri
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.viewModelScope
import io.github.dikshant363.pdfcraft.PdfcraftApplication
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.plural
import io.github.dikshant363.pdfcraft.text
import io.github.dikshant363.pdfcraft.core.MobileException
import io.github.dikshant363.pdfcraft.engine.DocumentFiles
import io.github.dikshant363.pdfcraft.engine.Engine
import io.github.dikshant363.pdfcraft.engine.RecentFile
import io.github.dikshant363.pdfcraft.engine.RecentFiles
import io.github.dikshant363.pdfcraft.engine.userMessage
import io.github.dikshant363.pdfcraft.ui.viewer.PageImages
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** A message for the snackbar, with an optional action. */
data class Message(val text: String, val actionLabel: String? = null, val action: (() -> Unit)? = null)

/** A document waiting for its password before it opens. */
data class PasswordRequest(val uri: Uri, val file: File, val wrong: Boolean)

/** Script alerts waiting at once, at most. */
private const val MAX_SCRIPT_ALERTS = 20

/** The saved-state key of the scan in progress. */
private const val SCAN_STATE = "scan"

/**
 * The app's state: the engine, the open document, Home's lists, and the messages. [saved] keeps
 * what must outlive Android closing the app in the background (the scan in progress).
 */
class AppViewModel(app: Application, private val saved: SavedStateHandle) : AndroidViewModel(app) {
    private val application = app as PdfcraftApplication
    val files = DocumentFiles(app)
    val recents = RecentFiles(File(app.filesDir, "recent.json"))

    var engine by mutableStateOf<Engine?>(null)
        private set
    var engineProblem by mutableStateOf<String?>(null)
        private set

    /** The document in the viewer, if one is open. */
    var document by mutableStateOf<OpenDocument?>(null)
        private set

    var recentList by mutableStateOf<List<RecentFile>>(emptyList())
        private set

    var passwordRequest by mutableStateOf<PasswordRequest?>(null)
        private set

    /** A document is being opened (a spinner over Home). */
    var opening by mutableStateOf(false)
        private set

    val messages = Channel<Message>(Channel.BUFFERED)

    /** How many bytes of page images to keep: an eighth of the app's heap, between 32 and 192 MB. */
    private val imageBudget: Int = run {
        val am = app.getSystemService(ActivityManager::class.java)
        val heapMb = (am?.largeMemoryClass ?: 256).coerceAtLeast(64)
        (heapMb / 4).coerceIn(32, 192) * 1024 * 1024
    }

    init {
        viewModelScope.launch {
            application.engine.await().onSuccess { engine = it }.onFailure { engineProblem = it.userMessage }
            refreshRecents()
            // the signers the reader trusts, before any signature is checked
            // (a document opened from another app meanwhile is checked again)
            engine?.let { eng ->
                if (withContext(Dispatchers.IO) { trustedCerts.files().isNotEmpty() }) {
                    runCatching { applyTrust(eng) }
                    document?.let { loadSignatures(eng, it) }
                }
            }
        }
    }

    fun tell(text: String, actionLabel: String? = null, action: (() -> Unit)? = null) {
        messages.trySend(Message(text, actionLabel, action))
    }

    fun refreshRecents() {
        viewModelScope.launch { recentList = withContext(Dispatchers.IO) { recents.list() } }
    }

    /** Open the PDF at [uri] (picked, shared, or from Recent), asking for its password if it has one. */
    fun open(uri: Uri, password: String? = null) {
        // a file opened from another app comes to the front, not under Settings
        settingsOpen = false
        showLicences = false
        viewModelScope.launch {
            val eng = engine ?: application.engine.await().getOrNull() ?: run { tell(engineProblem ?: text(R.string.msg_engine_not_started)); return@launch }
            opening = true
            try {
                val file = withContext(Dispatchers.IO) {
                    files.keepAccess(uri)
                    files.workingCopy(uri)
                }
                openFile(eng, uri, file, password)
            } catch (e: java.io.FileNotFoundException) {
                // moved or deleted since it was opened: it leaves Recent
                withContext(Dispatchers.IO) { recents.remove(setOf(uri.toString())) }
                refreshRecents()
                tell(text(R.string.msg_file_moved))
            } catch (e: SecurityException) {
                // A file another app lent for one visit (Files, Downloads: "Open with"): Android
                // takes that access back later, so Recent can't read it again. PdfCraft's own copy
                // opens instead, read-only (changes are offered as a copy, as for any file it
                // can't write to); the original needs opening again from the app it's in.
                val copy = withContext(Dispatchers.IO) { files.existingCopy(uri) }
                if (copy != null) {
                    openFile(eng, uri, copy, password)
                    tell(text(R.string.msg_opened_own_copy))
                } else {
                    tell(text(R.string.msg_file_access_lost))
                }
            } catch (e: Exception) {
                tell(e.userMessage)
            } finally {
                opening = false
            }
        }
    }

    /** The password dialog's answer: try again with [password], or give up (null). */
    fun answerPassword(password: String?) {
        val req = passwordRequest ?: return
        passwordRequest = null
        if (password == null) return
        // OK with nothing typed: the field turns red, as a wrong password does
        if (password.isEmpty()) {
            passwordRequest = PasswordRequest(req.uri, req.file, wrong = true)
            return
        }
        val eng = engine ?: return
        viewModelScope.launch {
            opening = true
            try {
                openFile(eng, req.uri, req.file, password)
            } finally {
                opening = false
            }
        }
    }

    private suspend fun openFile(eng: Engine, uri: Uri, file: File, password: String?) {
        try {
            document?.let { saveAndClose(eng, it) }
            val info = eng.command { open(file.absolutePath, password) }
            val images = PageImages(eng, info.id, viewModelScope, imageBudget)
            val source = uri.takeIf { it.scheme == "content" || it.scheme == "file" }
            val doc = OpenDocument(info.id, info, source, file, images, password, eng, viewModelScope)
            val (recent, writable) = withContext(Dispatchers.IO) {
                recents.list().firstOrNull { it.uri == uri.toString() } to (source == null || files.canWrite(source))
            }
            doc.writable = writable
            loadLinks(eng, doc)
            doc.controller.nightMode = savedNightMode
            if (savedPageMode != doc.viewer.mode) doc.viewer.switchMode(savedPageMode)
            // a file left in Reading Mode reopens in it, unless it was opened for a tool
            readingMode = recent?.reading == true && pendingTool == null
            document = doc
            if (doc.info.signed) announceSignatures(doc)
            startPendingTool(doc)
            pendingPage = recent?.lastPage?.takeIf { it in 0 until info.pages.size }
            pendingSpot = recent?.takeIf { pendingPage != null && it.lastX >= 0f && it.lastY >= 0f }?.let { Triple(it.lastX, it.lastY, it.lastZoom) }
            withContext(Dispatchers.IO) {
                recents.touch(RecentFile(uri.toString(), info.name, System.currentTimeMillis(), info.pages.size, file.length(), recent?.lastPage ?: 0))
            }
            refreshRecents()
        } catch (e: MobileException.NeedsPassword) {
            passwordRequest = PasswordRequest(uri, file, wrong = false)
        } catch (e: MobileException.WrongPassword) {
            passwordRequest = PasswordRequest(uri, file, wrong = true)
        } catch (e: MobileException) {
            tell(e.userMessage)
        }
    }

    /** The page to show once the viewer has laid the document out (reopening where the reader left off). */
    var pendingPage: Int? = null

    /** With [pendingPage]: the spot (points) at the screen's top-left and the zoom it was left at. */
    var pendingSpot: Triple<Float, Float, Float>? = null

    /** Intents for the activity to start (share, open a link): the view model has no activity. */
    val intents = Channel<Intent>(Channel.BUFFERED)

    /** Print jobs for the activity: the file and its name. */
    val prints = Channel<Pair<File, String>>(Channel.BUFFERED)

    /** A document that couldn't be saved back to where it came from, waiting for Save a copy or Discard. */
    var readOnlyPrompt by mutableStateOf<OpenDocument?>(null)
        private set

    /** Leave the viewer: changes are saved first, as the reference does (no prompt). */
    fun closeDocument() {
        val doc = document ?: return
        val eng = engine ?: return
        if (doc.controller.tool != ViewerTool.None) leaveTool(doc)
        val pendingCommit = commitFieldEdit()
        viewModelScope.launch {
            pendingCommit?.join()
            // changes the file's place can't take (known read-only, or found so now) are offered as a copy
            if ((doc.info.dirty || doc.notWrittenBack) && (!doc.writable || !save(doc, quiet = true))) {
                readOnlyPrompt = doc
                return@launch
            }
            document = null
            saveAndClose(eng, doc)
            readingMode = false
            refreshRecents()
        }
    }

    /** The read-only prompt's answer: keep the changes in a copy, or drop them. */
    fun answerReadOnly(saveCopy: Boolean?) {
        val doc = readOnlyPrompt ?: return
        readOnlyPrompt = null
        val eng = engine ?: return
        when (saveCopy) {
            null -> return
            true -> saveCopyAs(doc, then = {
                document = null
                viewModelScope.launch {
                    closeQuietly(eng, doc)
                    refreshRecents()
                }
            })
            false -> {
                document = null
                viewModelScope.launch {
                    closeQuietly(eng, doc)
                    refreshRecents()
                }
            }
        }
    }

    private suspend fun saveAndClose(eng: Engine, doc: OpenDocument) {
        val reading = readingMode
        val pending = pendingPage
        // the spot at the screen's top-left and the zoom, to reopen there (as the reference does)
        val v = doc.viewer
        val spot = if (pending == null && v.mode == io.github.dikshant363.pdfcraft.ui.viewer.PageMode.Continuous) v.screenToPage(androidx.compose.ui.geometry.Offset(0f, v.padTop), nearest = true) else null
        val zoom = v.zoom
        withContext(Dispatchers.IO) {
            // in Reading Mode the page canvas has no layout: the page read is the pending one
            val page = pending ?: spot?.page ?: v.currentPage
            doc.source?.let { src ->
                recents.update(src.toString()) {
                    it.copy(lastPage = page, reading = reading, lastX = spot?.x ?: -1f, lastY = spot?.y ?: -1f, lastZoom = if (spot != null) zoom else 1f)
                }
            }
        }
        save(doc)
        // Recent shows the size the file has now, not the size it had when opened
        withContext(Dispatchers.IO) {
            doc.source?.let { src -> files.size(src)?.let { size -> recents.update(src.toString()) { it.copy(size = size) } } }
        }
        closeQuietly(eng, doc)
    }

    private suspend fun closeQuietly(eng: Engine, doc: OpenDocument) {
        runCatching { eng.command { close(doc.id, true) } }
        doc.images.close()
        doc.search.clear()
    }

    /** The app went to the background: save the open document, as the reference does. */
    fun onPause() {
        val doc = document ?: return
        if (doc.controller.tool == ViewerTool.Draw) doc.commitTool?.invoke()
        viewModelScope.launch { save(doc) }
    }

    /**
     * Save [doc]'s changes: into its working file (incrementally, so earlier signatures stay
     * valid), then back over the file it came from when the app may write there. Returns
     * whether the changes reached the original; [quiet] leaves a failure's message to the caller.
     */
    suspend fun save(doc: OpenDocument, quiet: Boolean = false): Boolean {
        val eng = engine ?: return false
        if (!doc.info.dirty && !doc.notWrittenBack) return true
        return try {
            if (doc.info.dirty) eng.tool("doc_save", mapOf("doc" to doc.id))
            refresh(doc)
            val src = doc.source
            if (src != null && src.scheme == "content") {
                if (!doc.writable) {
                    doc.notWrittenBack = true
                    return false
                }
                withContext(Dispatchers.IO) { files.writeBack(doc.file, src) }
            } else if (src != null && src.scheme == "file") {
                val target = File(requireNotNull(src.path))
                if (target.canonicalPath != doc.file.canonicalPath) withContext(Dispatchers.IO) { doc.file.copyTo(target, overwrite = true) }
            }
            doc.notWrittenBack = false
            true
        } catch (e: Exception) {
            doc.writable = false
            doc.notWrittenBack = true
            if (!quiet) tell(e.userMessage)
            false
        }
    }

    /** Read the document's state from the engine again (after an edit, undo or save). */
    suspend fun refresh(doc: OpenDocument) {
        val eng = engine ?: return
        val info = runCatching { eng.command { document(doc.id) } }.getOrNull() ?: return
        val changed = info.generation != doc.info.generation
        doc.update(info)
        if (changed) loadLinks(eng, doc)
    }

    private suspend fun loadLinks(eng: Engine, doc: OpenDocument) {
        doc.widgets = runCatching { eng.command { widgets(doc.id) } }.getOrDefault(emptyList())
        doc.comments = runCatching { eng.command { comments(doc.id) } }.getOrDefault(emptyList())
        doc.links = runCatching {
            eng.command { (0 until doc.info.pages.size).associateWith { p -> links(doc.id, p.toUInt()) } }
        }.getOrDefault(emptyMap())
        loadSignatures(eng, doc)
    }

    /**
     * Run engine tool [name] on [doc] (its `doc` argument is filled in), then refresh. [onDone]
     * gets the result; a refusal is told in a snackbar.
     */
    fun runTool(doc: OpenDocument, name: String, args: Map<String, Any?> = emptyMap(), onDone: (JSONObject) -> Unit = {}) {
        val eng = engine ?: return
        viewModelScope.launch {
            try {
                val r = eng.tool(name, args + ("doc" to doc.id))
                refresh(doc)
                onDone(r)
            } catch (e: MobileException) {
                tell(e.userMessage)
            }
        }
    }

    /** Undo: inside a tool that keeps its own history (Draw) only that; else the document's last edit, named in a message. */
    fun undo(doc: OpenDocument) {
        doc.localUndo?.let { it.undo(); return }
        val label = doc.info.undo ?: return
        runTool(doc, "edit_undo") { tell(text(R.string.msg_undone, label)) }
    }

    fun redo(doc: OpenDocument) {
        doc.localUndo?.let { it.redo(); return }
        val label = doc.info.redo ?: return
        runTool(doc, "edit_redo") { tell(text(R.string.msg_redone, label)) }
    }

    /** Switch the viewer to [tool], unless the document forbids what it does. */
    fun enterTool(doc: OpenDocument, tool: ViewerTool) {
        val ctl = doc.controller
        if (ctl.tool == tool) {
            leaveTool(doc)
            return
        }
        // as the reference: no tool works on reflowed text; say so and stay in Reading Mode
        if (readingMode && tool != ViewerTool.None) {
            readingModeBlocked = tool
            return
        }
        val info = doc.info
        val blocked = when (tool) {
            ViewerTool.Comment, ViewerTool.Markup, ViewerTool.Draw, ViewerTool.Text -> !info.allowsAnnotation
            ViewerTool.FillSign -> !info.allowsFormFilling && !info.allowsAnnotation
            ViewerTool.Edit -> info.readOnlyReason != null || !info.allowsAssembly && info.encrypted
            ViewerTool.None -> false
        }
        if (blocked) {
            tell(info.readOnlyReason ?: text(R.string.msg_tool_blocked_security))
            return
        }
        if (ctl.tool != ViewerTool.None) leaveTool(doc)
        doc.selection.clear()
        ctl.switchTool(tool)
        if (tool == ViewerTool.Draw && toolPrefs.snapToShape && toolPrefs.takeSnapHint()) tell(text(R.string.msg_snap_to_shape_on))
        // the first time the reader comments without a name, ask for one (once)
        if (tool in setOf(ViewerTool.Comment, ViewerTool.Markup, ViewerTool.Draw, ViewerTool.Text) && toolPrefs.author.isBlank() && !toolPrefs.authorAsked) {
            toolPrefs.authorAsked = true
            authorPrompt = true
        }
    }

    /** Asking for the name comments are signed with. */
    var authorPrompt by mutableStateOf(false)

    /** Leave the active tool: whatever it holds (ink being drawn) is added first. */
    fun leaveTool(doc: OpenDocument) {
        doc.commitTool?.invoke()
        doc.commitTool = null
        if (doc.controller.tool == ViewerTool.Edit) {
            commitInlineEdit(doc)
            editSelection = null
            editImage = null
            editMode = io.github.dikshant363.pdfcraft.ui.viewer.EditMode.Select
        }
        if (certPlacing === doc) certPlacing = null
        doc.controller.tool = ViewerTool.None
    }

    /**
     * A tap at [at]: follow the link there, if any (a page of this document straight away, a
     * web address after asking). Returns whether there was one.
     */
    fun tapLink(doc: OpenDocument, at: PagePoint?): Boolean {
        if (at == null) return false
        val link = doc.links[at.page]?.firstOrNull { l ->
            l.rect.size == 4 && at.x in l.rect[0]..l.rect[2] && at.y in l.rect[1]..l.rect[3]
        } ?: return false
        doc.linkFlash = LinkFlash(at.page, link.rect.toFloatArray())
        viewModelScope.launch {
            delay(300)
            doc.linkFlash = null
        }
        val page = link.page
        val uri = link.uri
        when {
            page != null -> {
                doc.controller.rememberJump(doc.viewer.currentPage)
                doc.viewer.goToPage(page.toInt(), link.top)
            }
            uri != null -> {
                val parsed = Uri.parse(uri)
                when (parsed.scheme?.lowercase()) {
                    "mailto", "tel" -> intents.trySend(Intent(Intent.ACTION_VIEW, parsed))
                    else -> linkPrompt = uri
                }
            }
        }
        return true
    }

    /** A web address waiting for "Open web page?". */
    var linkPrompt by mutableStateOf<String?>(null)
        private set

    fun answerLink(open: Boolean) {
        val uri = linkPrompt ?: return
        linkPrompt = null
        if (open) intents.trySend(Intent(Intent.ACTION_VIEW, Uri.parse(uri)))
    }

    /** Find ended: say so when nothing was found. */
    fun whenSearchDone(doc: OpenDocument) {
        viewModelScope.launch {
            snapshotFlow { doc.search.running }.first { !it }
            if (doc.search.hits.isEmpty() && doc.search.query.isNotEmpty()) {
                val eng = engine
                val textless = eng != null && (0 until minOf(doc.pageCount, 5)).all { p ->
                    runCatching { withContext(eng.commands) { eng.session.glyphCount(doc.id, p.toUInt()) } }.getOrDefault(1u) == 0u
                }
                if (textless) {
                    tell(text(R.string.msg_search_image_only), text(R.string.common_tool_recognize_text)) { recognizeText(doc) }
                } else {
                    tell(text(R.string.msg_search_no_matches, doc.search.query))
                }
            }
        }
    }

    /** Share a copy of [doc] through the system's share sheet (saved first, with the ink being drawn). */
    fun share(doc: OpenDocument) {
        if (doc.controller.tool == ViewerTool.Draw) doc.commitTool?.invoke()
        handOut {
            // the ink is queued in the engine: reading the document back waits for it, so the save has it
            refresh(doc)
            save(doc)
            val uri = withContext(Dispatchers.IO) { files.shareable(doc.file, doc.name) }
            sendIntent(listOf(uri)).putExtra(Intent.EXTRA_SUBJECT, text(R.string.msg_share_subject, doc.title))
        }
    }

    /**
     * Hand files to another app through the system's chooser: [make] copies them where other apps
     * may read them and returns the intent. A failure on the way, such as a file gone or a phone
     * too full for the copy, is told in a snackbar instead of closing the app.
     */
    private fun handOut(make: suspend () -> Intent) {
        viewModelScope.launch {
            val intent = try {
                make()
            } catch (e: Exception) {
                tell(e.userMessage)
                null
            } ?: return@launch
            intents.trySend(Intent.createChooser(intent, null))
        }
    }

    /** A send intent for [uris], with them as its clip too so the share sheet can show them. */
    private fun sendIntent(uris: List<Uri>, mime: String = "application/pdf"): Intent {
        val send = if (uris.size == 1) Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_STREAM, uris.first())
        else Intent(Intent.ACTION_SEND_MULTIPLE).putParcelableArrayListExtra(Intent.EXTRA_STREAM, ArrayList(uris))
        send.clipData = ClipData(null, arrayOf(mime), ClipData.Item(uris.first())).apply { uris.drop(1).forEach { addItem(ClipData.Item(it)) } }
        return send.setType(mime).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }

    /**
     * Keys in the viewer, as the reference takes them: N or Page Down for the next page, P or Page
     * Up for the previous one, the arrows to scroll, Ctrl+P to print, Ctrl and + or − to zoom.
     */
    fun viewerKey(keyCode: Int, ctrl: Boolean): Boolean {
        if (!keyboardShortcuts) return false
        val doc = document ?: return false
        if (toolScreen != null || readingMode) return false
        val v = doc.viewer
        val center = androidx.compose.ui.geometry.Offset(v.viewW / 2f, v.viewH / 2f)
        when {
            ctrl && keyCode == android.view.KeyEvent.KEYCODE_P -> print(doc)
            ctrl && (keyCode == android.view.KeyEvent.KEYCODE_EQUALS || keyCode == android.view.KeyEvent.KEYCODE_PLUS || keyCode == android.view.KeyEvent.KEYCODE_NUMPAD_ADD) -> v.zoomStep(1, center)
            ctrl && (keyCode == android.view.KeyEvent.KEYCODE_MINUS || keyCode == android.view.KeyEvent.KEYCODE_NUMPAD_SUBTRACT) -> v.zoomStep(-1, center)
            ctrl -> return false
            keyCode == android.view.KeyEvent.KEYCODE_N || keyCode == android.view.KeyEvent.KEYCODE_PAGE_DOWN -> v.goToPage(v.currentPage + 1)
            keyCode == android.view.KeyEvent.KEYCODE_P || keyCode == android.view.KeyEvent.KEYCODE_PAGE_UP -> v.goToPage(v.currentPage - 1)
            keyCode == android.view.KeyEvent.KEYCODE_DPAD_DOWN -> v.setScroll(v.scrollX, v.scrollY + v.viewH * 0.1f)
            keyCode == android.view.KeyEvent.KEYCODE_DPAD_UP -> v.setScroll(v.scrollX, v.scrollY - v.viewH * 0.1f)
            keyCode == android.view.KeyEvent.KEYCODE_DPAD_RIGHT -> v.setScroll(v.scrollX + v.viewW * 0.1f, v.scrollY)
            keyCode == android.view.KeyEvent.KEYCODE_DPAD_LEFT -> v.setScroll(v.scrollX - v.viewW * 0.1f, v.scrollY)
            else -> return false
        }
        return true
    }

    /** Print [doc] through Android's print framework. */
    fun print(doc: OpenDocument) {
        if (!doc.info.allowsPrinting) {
            tell(text(R.string.msg_print_disabled))
            return
        }
        viewModelScope.launch {
            save(doc)
            // the system's printing can't open a password-protected file: print an unlocked copy
            val file = if (doc.info.encrypted) unlockedCopy(doc) ?: doc.file else doc.file
            prints.trySend(file to doc.name)
        }
    }

    /** A temporary copy of [doc] without its password, for printing; null if the security can't be removed. */
    private suspend fun unlockedCopy(doc: OpenDocument): File? {
        val eng = engine ?: return null
        return runCatching {
            val out = tempFile("pdf")
            val copy = eng.tool("doc_open", mapOf("path" to doc.file.absolutePath, "password" to doc.password)).optLong("doc")
            try {
                eng.tool("doc_unprotect", mapOf("doc" to copy))
                eng.tool("doc_save", mapOf("doc" to copy, "path" to out.absolutePath, "full" to true))
            } finally {
                runCatching { eng.tool("doc_close", mapOf("doc" to copy, "discard_changes" to true)) }
            }
            out
        }.getOrNull()
    }

    /** Where Save a copy is asked to write (the activity opens the system's file creator with this name). */
    val saveCopyRequests = Channel<String>(Channel.BUFFERED)
    private var afterCopy: (() -> Unit)? = null
    private var copyOf: OpenDocument? = null

    /** Save a copy of [doc] under a name and place the user picks. */
    fun saveCopyAs(doc: OpenDocument, then: (() -> Unit)? = null) {
        copyOf = doc
        afterCopy = then
        saveCopyRequests.trySend(doc.name)
    }

    /** The place chosen for Save a copy (null: cancelled). The document is saved first, then its file copied. */
    fun copyTo(target: Uri?) {
        if (rowCopyTo(target)) return
        val doc = copyOf ?: return
        copyOf = null
        val then = afterCopy
        afterCopy = null
        if (target == null) return
        viewModelScope.launch {
            try {
                // a read-only source can't take the changes, but the working copy has them all
                if (doc.info.dirty) engine?.tool("doc_save", mapOf("doc" to doc.id))
                withContext(Dispatchers.IO) { files.writeBack(doc.file, target) }
                refresh(doc)
                tell(text(R.string.msg_saved_copy))
                then?.invoke()
            } catch (e: Exception) {
                tell(e.userMessage)
            }
        }
    }

    /** Export PDF (to Word, RTF, Excel, PowerPoint, or images of its pages). */
    fun openExport(doc: OpenDocument) {
        toolScreen = ToolScreen.Export(doc)
    }

    /** Read Aloud: the player over the viewer. */
    var readAloud by mutableStateOf<ReadAloud?>(null)
        private set

    /** The reading notification follows Read aloud: shown while it reads or is paused, gone when it stops. */
    private val readAloudNotice = viewModelScope.launch {
        snapshotFlow { readAloud to (readAloud?.playing == true) }.collect { (ra, playing) ->
            val ctx = getApplication<Application>()
            if (ra == null) {
                io.github.dikshant363.pdfcraft.ReadAloudService.controls = null
                io.github.dikshant363.pdfcraft.ReadAloudService.hide(ctx)
            } else {
                io.github.dikshant363.pdfcraft.ReadAloudService.controls = object : io.github.dikshant363.pdfcraft.ReadAloudService.Controls {
                    override fun play() { readAloud?.play() }
                    override fun pause() { readAloud?.pause() }
                    override fun skip(forward: Boolean) { readAloud?.skip(forward) }
                    override fun stop() { readAloud?.stop() }
                }
                io.github.dikshant363.pdfcraft.ReadAloudService.show(ctx, ra.title, playing)
            }
        }
    }

    fun startReadAloud(doc: OpenDocument, fromSelection: Boolean = false) {
        if (doc.info.encrypted) {
            tell(text(R.string.msg_read_aloud_encrypted))
            return
        }
        readAloud?.stop()
        val eng = engine ?: return
        val key = docKey(doc)
        val sel = if (fromSelection) doc.selection.selection?.range else null
        if (sel != null) {
            // from the selection's first word
            readAloud = ReadAloud(getApplication(), eng, doc, viewModelScope, sel.page.toInt(), { readAloud = null }, key, startGlyph = sel.start)
            return
        }
        val saved = ReadAloud.savedPosition(getApplication(), key)?.takeIf { it.first in 0 until doc.pageCount }
        // a reading that went to the end starts over at the beginning, as the reference's does
        if (saved == ReadAloud.START_OVER) {
            readAloud = ReadAloud(getApplication(), eng, doc, viewModelScope, 0, { readAloud = null }, key)
            return
        }
        if (saved != null) {
            readAloud = ReadAloud(getApplication(), eng, doc, viewModelScope, saved.first, { readAloud = null }, key, resumeAt = saved.second to saved.third)
            tell(text(R.string.msg_read_aloud_resumed), text(R.string.msg_read_aloud_start_over)) {
                ReadAloud.forgetPosition(getApplication(), key)
                readAloud?.stop()
                ReadAloud.forgetPosition(getApplication(), key)
                readAloud = ReadAloud(getApplication(), eng, doc, viewModelScope, 0, { readAloud = null }, key)
            }
            return
        }
        readAloud = ReadAloud(getApplication(), eng, doc, viewModelScope, doc.viewer.currentPage, { readAloud = null }, key)
    }

    /**
     * While Read aloud is on, a tap on a word moves the reading there, as the reference's does. False
     * when no word is under [at] (the tap then shows or hides the bars).
     */
    suspend fun readAloudAt(doc: OpenDocument, at: io.github.dikshant363.pdfcraft.ui.viewer.PagePoint, slopPoints: Float): Boolean {
        val current = readAloud ?: return false
        val eng = engine ?: return false
        val word = withContext(eng.commands) { eng.session.wordAt(doc.id, at.page.toUInt(), at.x, at.y, slopPoints) } ?: return false
        if (readAloud !== current) return false
        current.stop()
        readAloud = ReadAloud(getApplication(), eng, doc, viewModelScope, word.page.toInt(), { readAloud = null }, docKey(doc), startGlyph = word.start)
        return true
    }

    // ---- Tools' remembered settings, bookmarks, stars --------------------------------------

    val toolPrefs = ToolPrefs(app)

    /** The viewer's bottom toolbar as the user arranged it (More tools ▸ Customize toolbar). */
    val quickToolbar = io.github.dikshant363.pdfcraft.ui.viewer.QuickToolbarPrefs(app)
    private val appPrefs = app.getSharedPreferences("app", android.content.Context.MODE_PRIVATE)
    private val bookmarkStore = UserBookmarks(File(app.filesDir, "bookmarks.json"))

    /** The Search tab's last searches, newest first (kept on the phone, as the reference keeps them). */
    var recentSearches by mutableStateOf(appPrefs.getString("recentSearches", null)?.split('\n')?.filter { it.isNotBlank() } ?: emptyList())
        private set

    fun addRecentSearch(q: String) {
        val t = q.trim()
        if (t.isEmpty()) return
        recentSearches = (listOf(t) + recentSearches.filterNot { it.equals(t, ignoreCase = true) }).take(10)
        appPrefs.edit().putString("recentSearches", recentSearches.joinToString("\n")).apply()
    }

    fun removeRecentSearch(q: String) {
        recentSearches = recentSearches - q
        appPrefs.edit().putString("recentSearches", recentSearches.joinToString("\n")).apply()
    }

    fun clearRecentSearches() {
        recentSearches = emptyList()
        appPrefs.edit().remove("recentSearches").apply()
    }

    /** The key a document's own data (bookmarks, stars, last page) is kept under: where it came from. */
    fun docKey(doc: OpenDocument): String = doc.source?.toString() ?: doc.file.absolutePath

    /** The reader's bookmarks of a document. */
    inner class Bookmarks {
        fun list(doc: OpenDocument) = bookmarkStore.list(docKey(doc))
        fun isBookmarked(doc: OpenDocument, page: Int) = bookmarkStore.has(docKey(doc), page)
        fun rename(doc: OpenDocument, b: UserBookmark, name: String) = bookmarkStore.rename(docKey(doc), b, name)
        fun remove(doc: OpenDocument, b: UserBookmark) = bookmarkStore.remove(docKey(doc), b)
        fun clear(doc: OpenDocument) = bookmarkStore.clear(docKey(doc))
    }

    val bookmarks = Bookmarks()

    /** Add a bookmark on the current page, or remove the one there. */
    fun toggleBookmark(doc: OpenDocument) {
        val (b, added) = bookmarkStore.toggle(docKey(doc), doc.viewer.currentPage)
        tell(text(if (added) R.string.msg_bookmark_added else R.string.msg_bookmark_removed, b.name))
    }

    /** Delete all bookmarks of a document: waiting for confirmation. */
    var confirmDeleteAllBookmarks by mutableStateOf<OpenDocument?>(null)

    fun deleteAllBookmarks(doc: OpenDocument) {
        bookmarks.clear(doc)
        tell(text(R.string.msg_bookmarks_cleared))
    }

    fun isStarred(doc: OpenDocument): Boolean = recentList.firstOrNull { it.uri == docKey(doc) }?.starred == true

    fun toggleStar(doc: OpenDocument) {
        val key = docKey(doc)
        viewModelScope.launch {
            withContext(Dispatchers.IO) { recents.update(key) { it.copy(starred = !it.starred) } }
            refreshRecents()
        }
    }

    fun setStarred(uri: String, starred: Boolean) {
        viewModelScope.launch {
            withContext(Dispatchers.IO) { recents.update(uri) { it.copy(starred = starred) } }
            refreshRecents()
        }
    }

    // ---- View settings ----------------------------------------------------------------------

    /** Reading Mode (text reflowed to the screen) is showing. Remembered for each file, as the reference does. */
    var readingMode by mutableStateOf(false)
        private set

    /** The page mode documents open in (View settings; remembered app-wide). */
    val savedPageMode: io.github.dikshant363.pdfcraft.ui.viewer.PageMode
        get() = if (appPrefs.getString("pageMode", "continuous") == "single") io.github.dikshant363.pdfcraft.ui.viewer.PageMode.SinglePage
        else io.github.dikshant363.pdfcraft.ui.viewer.PageMode.Continuous

    val savedNightMode: Boolean get() = appPrefs.getBoolean("nightMode", false)

    fun setPageMode(doc: OpenDocument, mode: io.github.dikshant363.pdfcraft.ui.viewer.PageMode) {
        // leaving Reading Mode: a canvas laid out already followed the page read; else it shows it on layout
        if (readingMode && doc.viewer.viewW > 0f) pendingPage = null
        readingMode = false
        doc.viewer.switchMode(mode)
        appPrefs.edit().putString("pageMode", if (mode == io.github.dikshant363.pdfcraft.ui.viewer.PageMode.SinglePage) "single" else "continuous").apply()
    }

    fun setNightMode(doc: OpenDocument, on: Boolean) {
        doc.controller.nightMode = on
        appPrefs.edit().putBoolean("nightMode", on).apply()
    }

    fun openReadingMode(doc: OpenDocument) {
        if (doc.controller.tool != ViewerTool.None) leaveTool(doc)
        readingMode = true
    }

    /** A tool tried in Reading Mode, for the dialog that says it needs another view. */
    var readingModeBlocked by mutableStateOf<ViewerTool?>(null)

    /** Keep the screen on while reading (Preferences; released after half an hour without a touch). */
    var keepScreenOn by mutableStateOf(appPrefs.getBoolean("keepScreenOn", false))
        private set

    fun updateKeepScreenOn(on: Boolean) {
        keepScreenOn = on
        appPrefs.edit().putBoolean("keepScreenOn", on).apply()
    }

    /** The viewer's keyboard shortcuts (Preferences; off until turned on, as the reference has them). */
    var keyboardShortcuts by mutableStateOf(appPrefs.getBoolean("keyboardShortcuts", false))
        private set

    fun updateKeyboardShortcuts(on: Boolean) {
        keyboardShortcuts = on
        appPrefs.edit().putBoolean("keyboardShortcuts", on).apply()
    }

    /** The app's theme (Preferences ▸ Theme): the system's, or always light or dark. */
    var theme by mutableStateOf(runCatching { ThemeChoice.valueOf(appPrefs.getString("theme", "System") ?: "System") }.getOrDefault(ThemeChoice.System))
        private set

    fun setThemeChoice(t: ThemeChoice) {
        theme = t
        appPrefs.edit().putString("theme", t.name).apply()
    }

    /**
     * Extra Features ▸ the status bar in documents: shown with the viewer's bars, and the page uses the
     * whole screen when they hide (off: the reference's black band where the camera is).
     */
    var statusBarInDocuments by mutableStateOf(appPrefs.getBoolean("statusBarInDocuments", false))
        private set

    fun updateStatusBarInDocuments(on: Boolean) {
        statusBarInDocuments = on
        appPrefs.edit().putBoolean("statusBarInDocuments", on).apply()
    }

    /**
     * Extra Features ▸ the number of search results: Find shows which result is on screen and how
     * many there are, "3 of 12" (#39; the reference's page view shows no count).
     */
    var searchCount by mutableStateOf(appPrefs.getBoolean("searchCount", false))
        private set

    fun updateSearchCount(on: Boolean) {
        searchCount = on
        appPrefs.edit().putBoolean("searchCount", on).apply()
    }

    // ---- Reading the document ---------------------------------------------------------------

    suspend fun outline(doc: OpenDocument) = engine?.let { e -> runCatching { e.command { outline(doc.id) } }.getOrNull() }.orEmpty()

    suspend fun attachments(doc: OpenDocument) = engine?.let { e -> runCatching { e.command { attachments(doc.id) } }.getOrNull() }.orEmpty()

    suspend fun comments(doc: OpenDocument) = engine?.let { e -> runCatching { e.command { comments(doc.id) } }.getOrNull() }.orEmpty()

    /** Open an embedded file: a PDF in the viewer, anything else in another app. */
    fun openAttachment(doc: OpenDocument, name: String) {
        val eng = engine ?: return
        viewModelScope.launch {
            try {
                val list = eng.command { attachments(doc.id) }
                val index = list.indexOfFirst { it.name == name }.takeIf { it >= 0 } ?: return@launch
                val dir = File(getApplication<Application>().cacheDir, "attachments/${System.nanoTime()}").apply { mkdirs() }
                val out = File(dir, DocumentFiles.safeName(name).let { if (name.lowercase().endsWith(".pdf")) it else name.replace(Regex("[\\\\/:*?\"<>|]"), "_") })
                eng.command { saveAttachment(doc.id, index.toUInt(), out.absolutePath) }
                if (out.name.lowercase().endsWith(".pdf")) {
                    open(Uri.fromFile(out))
                } else {
                    val uri = files.shareable(out, out.name)
                    val type = android.webkit.MimeTypeMap.getSingleton().getMimeTypeFromExtension(out.extension.lowercase()) ?: "*/*"
                    intents.trySend(Intent.createChooser(Intent(Intent.ACTION_VIEW).setDataAndType(uri, type).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION), null))
                }
            } catch (e: Exception) {
                tell(e.userMessage)
            }
        }
    }

    // ---- Comments ----------------------------------------------------------------------------

    /** A note being written (Comment tool, or Add comment on a selection). */
    var notePrompt by mutableStateOf<NotePrompt?>(null)

    /** Free text being typed (Text tool). */
    var textPrompt by mutableStateOf<TextPrompt?>(null)

    /** A reply being written to a comment. */
    var replyPrompt by mutableStateOf<ReplyPrompt?>(null)

    private val author: String? get() = toolPrefs.author.ifBlank { null }

    /** Mark text with [markup]: [rects] (points, one per line) on [page]. */
    fun addMarkup(doc: OpenDocument, markup: TextMarkup, page: Int, rects: List<FloatArray>, color: androidx.compose.ui.graphics.Color, opacity: Float, contents: String? = null) {
        val quads = rects.map { r -> listOf(r[0], r[1], r[2], r[1], r[0], r[3], r[2], r[3]).map { it.toDouble() } }
        runTool(
            doc, "comment_add",
            mapOf(
                "page" to page + 1, "type" to markup.engineType, "quads" to quads,
                "color" to color.hex(), "opacity" to opacity.toDouble(), "author" to author, "contents" to contents,
            ),
        )
    }

    /** Post a note at [at] (or, for a selection, a highlight carrying the note). */
    fun postNote(prompt: NotePrompt, text: String) {
        notePrompt = null
        val doc = prompt.doc
        val sel = prompt.selection
        if (sel != null) {
            addMarkup(doc, TextMarkup.Highlight, sel.range.page.toInt(), sel.rects, toolPrefs.color(StyledTool.Highlight), toolPrefs.opacity(StyledTool.Highlight), text)
            doc.selection.clear()
            return
        }
        val at = prompt.at ?: return
        runTool(
            doc, "comment_add",
            mapOf(
                "page" to at.page + 1, "type" to "note", "at" to listOf(at.x.toDouble(), at.y.toDouble()),
                "contents" to text, "color" to toolPrefs.color(StyledTool.Note).hex(), "author" to author,
            ),
        )
    }

    /** Place typed text at the prompt's point, in the Text tool's colour and size. */
    fun postText(prompt: TextPrompt, text: String) {
        textPrompt = null
        if (text.isBlank()) return
        val at = prompt.at
        val size = toolPrefs.textSize
        val page = prompt.doc.info.pages.getOrNull(at.page) ?: return
        // as the reference: lines that wrap where the page ends become real line breaks, and the box
        // is as wide as its longest line (measured as the engine draws it, so it doesn't wrap again)
        val (lines, width, height) = FreeTextLayout.layout(text, size, (page.width - at.x - 6f).coerceAtLeast(size * 4))
        val wrapped = lines.joinToString("\n")
        val x1 = (at.x + width).coerceAtMost(page.width)
        val y1 = (at.y + height).coerceAtMost(page.height)
        runTool(
            prompt.doc, "comment_add",
            mapOf(
                "page" to at.page + 1, "type" to "textbox", "rect" to listOf(at.x, at.y, x1, y1).map { it.toDouble() },
                "contents" to wrapped, "color" to toolPrefs.color(StyledTool.FreeText).hex(), "font_size" to size.toDouble(), "author" to author,
            ),
        )
    }

    // ---- Voice comments ----------------------------------------------------------------------

    /** A voice comment being recorded where the page was tapped. */
    var voicePrompt by mutableStateOf<VoicePrompt?>(null)

    /** Asks the activity for the microphone. */
    val micRequests = Channel<Unit>(Channel.CONFLATED)

    /** The microphone was refused before: explain, and offer the app's settings. */
    var micRationale by mutableStateOf(false)
    private var pendingVoice: VoicePrompt? = null

    /** Record a voice comment at [at], once the microphone is allowed. */
    fun startVoice(doc: OpenDocument, at: PagePoint) {
        val app = getApplication<Application>()
        if (app.cacheDir.usableSpace < 60L * 1024 * 1024) {
            tell(text(R.string.msg_voice_no_storage))
            return
        }
        val prompt = VoicePrompt(doc, at)
        if (androidx.core.content.ContextCompat.checkSelfPermission(app, android.Manifest.permission.RECORD_AUDIO) == android.content.pm.PackageManager.PERMISSION_GRANTED) {
            voicePrompt = prompt
        } else {
            pendingVoice = prompt
            micRequests.trySend(Unit)
        }
    }

    /** The answer to the microphone request. */
    fun onMicPermission(granted: Boolean) {
        val p = pendingVoice
        pendingVoice = null
        if (granted && p != null) voicePrompt = p else if (!granted) tell(text(R.string.msg_voice_no_mic))
    }

    fun dismissMicRationale(openSettings: Boolean) {
        micRationale = false
        pendingVoice = null
        if (openSettings) {
            val app = getApplication<Application>()
            intents.trySend(Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", app.packageName, null)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        }
    }

    /** A cache file for a new recording. */
    fun voiceFile(): File = File(getApplication<Application>().cacheDir, "voice-${System.nanoTime()}.wav")

    /** Post [file] (a WAV recording) as a voice comment at the prompt's point. */
    fun postVoice(prompt: VoicePrompt, file: File) {
        voicePrompt = null
        val at = prompt.at
        runTool(
            prompt.doc, "comment_add",
            mapOf(
                "page" to at.page + 1, "type" to "sound", "at" to listOf(at.x.toDouble(), at.y.toDouble()),
                "path" to file.absolutePath, "color" to toolPrefs.color(StyledTool.Note).hex(), "author" to author,
            ),
        ) { file.delete() }
    }

    /** Voice comment [c]'s recording as a WAV file to play, or null when it can't be read. */
    suspend fun voiceRecording(doc: OpenDocument, c: io.github.dikshant363.pdfcraft.core.CommentInfo): File? {
        val eng = engine ?: return null
        val f = File(getApplication<Application>().cacheDir, "voice-play-${doc.id}-${c.page}-${c.index}.wav")
        return withContext(eng.commands) {
            runCatching { eng.session.saveSound(doc.id, c.page, c.index, f.absolutePath); f }.getOrNull()
        }
    }

    fun postReply(prompt: ReplyPrompt, text: String) {
        replyPrompt = null
        if (text.isBlank()) return
        runTool(prompt.doc, "comment_reply", mapOf("page" to prompt.comment.page.toInt() + 1, "index" to prompt.comment.index.toInt() + 1, "text" to text, "author" to author))
    }

    /** Put [text] on the clipboard (Android 13 and later confirm it themselves). */
    fun copyText(text: String) {
        val cm = getApplication<Application>().getSystemService(android.content.ClipboardManager::class.java)
        // the parameter `text` hides the text() function here: called by its full name
        cm?.setPrimaryClip(android.content.ClipData.newPlainText(io.github.dikshant363.pdfcraft.text(R.string.msg_clip_comment), text))
        if (android.os.Build.VERSION.SDK_INT < 33) tell(io.github.dikshant363.pdfcraft.text(R.string.msg_copied))
    }

    /** Copy the selected text (when the document allows copying). */
    fun copySelection(doc: OpenDocument) {
        if (!doc.info.allowsCopying) {
            tell(text(R.string.msg_copy_not_allowed))
            return
        }
        viewModelScope.launch {
            val text = doc.selection.text() ?: return@launch
            val cm = getApplication<Application>().getSystemService(android.content.ClipboardManager::class.java)
            cm?.setPrimaryClip(android.content.ClipData.newPlainText(io.github.dikshant363.pdfcraft.text(R.string.msg_clip_pdf_text), text))
            doc.selection.clear()
            if (android.os.Build.VERSION.SDK_INT < 33) tell(io.github.dikshant363.pdfcraft.text(R.string.msg_copied))
        }
    }

    // ---- Tool screens (Compress, Combine, Protect, Create, Crop, Recognize text, Scan, Redact, Organize) ----

    /** The full-screen tool over the app, if one is open. */
    var toolScreen by mutableStateOf<ToolScreen?>(null)

    /** The Pages screen is organizing (not just showing thumbnails). */
    var organizing by mutableStateOf(false)

    fun openCompress(doc: OpenDocument) { toolScreen = ToolScreen.Compress(doc) }
    fun openCombine(doc: OpenDocument?) {
        // like the reference, the open document is the first file
        if (doc != null && combineFiles.isEmpty()) {
            val uri = doc.source ?: Uri.fromFile(doc.file)
            combineFiles.add(PickedFile(uri, doc.name, doc.info.fileSize.toLong()))
        }
        toolScreen = ToolScreen.Combine(doc)
    }
    fun openProtect(doc: OpenDocument) { toolScreen = ToolScreen.Protect(doc) }
    /** Create PDF starts on its own page of sources (photos, files, a scan), as the reference's. */
    fun openCreate() { toolScreen = ToolScreen.Create }
    fun openCrop(doc: OpenDocument) { toolScreen = ToolScreen.Crop(doc) }
    fun startScan() { toolScreen = ToolScreen.Scan }

    /** Recognize text: make scanned pages searchable and selectable. */
    fun recognizeText(doc: OpenDocument) {
        val eng = engine ?: return
        viewModelScope.launch {
            busy = text(R.string.msg_busy_preparing_ocr)
            withContext(Dispatchers.IO) { runCatching { eng.prepareOcrModels(getApplication()) } }
            busy = null
            val status = runCatching { eng.tool("ocr_status") }.getOrNull()
            if (status?.optBoolean("available", false) != true) {
                tell(text(R.string.msg_ocr_not_installed))
                return@launch
            }
            busy = text(R.string.msg_busy_recognizing)
            try {
                val r = eng.tool("ocr_recognize", mapOf("doc" to doc.id))
                refresh(doc)
                val pages = r.optJSONArray("pages")
                val n = (0 until (pages?.length() ?: 0)).count { pages?.optJSONObject(it)?.has("skipped") == false }
                if (n > 0) {
                    success = SuccessBanner(text(R.string.msg_ocr_done)) { undo(doc) }
                } else {
                    tell(text(R.string.msg_ocr_nothing))
                }
            } catch (e: MobileException) {
                tell(e.userMessage)
            } finally {
                busy = null
            }
        }
    }

    /** A finished job's green banner over the viewer's bar, with Undo. */
    var success by mutableStateOf<SuccessBanner?>(null)

    /** A long-running job's message (a progress dialog over everything), or null. */
    var busy by mutableStateOf<String?>(null)

    // ---- Fill & Sign -------------------------------------------------------------------------

    /** The element a tap places. */
    var fillElement by mutableStateOf(io.github.dikshant363.pdfcraft.ui.viewer.FillElement.Text)

    /** Fill & Sign text waiting to be typed. */
    var fillTextPrompt by mutableStateOf<TextPrompt?>(null)

    val signatures = io.github.dikshant363.pdfcraft.ui.viewer.Signatures(app.filesDir)

    /** Bumped when a saved signature is cleared or replaced, so the panel redraws. */
    var signaturesChanged by mutableStateOf(0)

    /** A signature or initials being made. */
    var signaturePad by mutableStateOf<io.github.dikshant363.pdfcraft.ui.viewer.SignaturePad?>(null)

    /** A signature taken from an image (Image tab), before Done. */
    var importedSignature by mutableStateOf<android.graphics.Bitmap?>(null)

    /** Place [element] at [at] on its page. */
    fun placeFill(doc: OpenDocument, element: io.github.dikshant363.pdfcraft.ui.viewer.FillElement, at: PagePoint, text: String? = null) {
        if (element == FillElement.Box) {
            // a rounded box around what was tapped, in the Fill & Sign colour: drawn as one ink stroke
            val r = toolPrefs.fillTextSize
            runTool(doc, "comment_add", mapOf(
                "page" to at.page + 1, "type" to "ink",
                "strokes" to listOf(roundedBox(at.x, at.y, r * 4f, r * 2f, r * 0.6f)),
                "color" to androidx.compose.ui.graphics.Color(toolPrefs.fillColor).hex(), "width" to 1.5, "author" to author,
            ))
            return
        }
        val args = mutableMapOf<String, Any?>(
            "page" to at.page + 1, "type" to element.engineType, "at" to listOf(at.x.toDouble(), at.y.toDouble()), "author" to author,
        )
        when (element) {
            FillElement.Text -> args["text"] = text ?: return
            FillElement.Signature, FillElement.Initials -> args["path"] = signatures.fileFor(element == FillElement.Initials).absolutePath
            else -> Unit
        }
        runTool(doc, "fill_sign_add", args)
    }

    /** The outline of a [w] x [h] box with corners of radius [r], centred on ([cx], [cy]), as stroke points. */
    private fun roundedBox(cx: Float, cy: Float, w: Float, h: Float, r: Float): List<List<Double>> {
        val pts = mutableListOf<List<Double>>()
        val l = cx - w / 2f
        val t = cy - h / 2f
        val corners = listOf(Triple(l + w - r, t + r, -90f), Triple(l + w - r, t + h - r, 0f), Triple(l + r, t + h - r, 90f), Triple(l + r, t + r, 180f))
        for ((x, y, start) in corners) {
            for (k in 0..6) {
                val a = Math.toRadians((start + k * 15f).toDouble())
                pts += listOf(x + r * Math.cos(a), y + r * Math.sin(a))
            }
        }
        pts += pts.first()
        return pts
    }

    fun postFillText(prompt: TextPrompt, text: String) {
        fillTextPrompt = null
        if (text.isBlank()) return
        placeFill(prompt.doc, io.github.dikshant363.pdfcraft.ui.viewer.FillElement.Text, prompt.at, text)
    }

    /** A signature or initials was made: save it (if asked) and place it where the page was tapped. */
    fun finishSignature(pad: io.github.dikshant363.pdfcraft.ui.viewer.SignaturePad, bitmap: android.graphics.Bitmap, save: Boolean) {
        signaturePad = null
        importedSignature = null
        viewModelScope.launch {
            val target = if (save) signatures.fileFor(pad.initials) else File(getApplication<Application>().cacheDir, "sign-${System.nanoTime()}.png")
            withContext(Dispatchers.IO) { target.outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) } }
            signaturesChanged++
                fillElement = if (pad.initials) FillElement.Initials else FillElement.Signature
            val doc = pad.doc
            val at = pad.at
            if (doc != null && at != null) {
                runTool(doc, "fill_sign_add", mapOf(
                    "page" to at.page + 1, "type" to if (pad.initials) "initials" else "signature",
                    "at" to listOf(at.x.toDouble(), at.y.toDouble()), "path" to target.absolutePath, "author" to author,
                ))
            } else {
                tell(text(if (pad.initials) R.string.msg_place_initials else R.string.msg_place_signature))
            }
        }
    }

    /** An image chosen for a signature: made transparent where it is near white, then shown on the pad. */
    fun importSignatureImage(uri: Uri?) {
        if (uri == null) return
        viewModelScope.launch {
            importedSignature = withContext(Dispatchers.IO) {
                runCatching {
                    val src = getApplication<Application>().contentResolver.openInputStream(uri).use { android.graphics.BitmapFactory.decodeStream(it) } ?: return@runCatching null
                    val scale = minOf(1f, 1200f / maxOf(src.width, src.height))
                    val bmp = android.graphics.Bitmap.createScaledBitmap(src, (src.width * scale).toInt().coerceAtLeast(1), (src.height * scale).toInt().coerceAtLeast(1), true)
                        .copy(android.graphics.Bitmap.Config.ARGB_8888, true)
                    val px = IntArray(bmp.width * bmp.height)
                    bmp.getPixels(px, 0, bmp.width, 0, 0, bmp.width, bmp.height)
                    for (i in px.indices) {
                        val p = px[i]
                        val l = (((p shr 16) and 0xFF) * 3 + ((p shr 8) and 0xFF) * 6 + (p and 0xFF)) / 10
                        // paper becomes transparent, ink keeps its strength
                        val a = ((235 - l) * 255 / 120).coerceIn(0, 255)
                        px[i] = (a shl 24) or (p and 0xFFFFFF)
                    }
                    bmp.setPixels(px, 0, bmp.width, 0, 0, bmp.width, bmp.height)
                    bmp
                }.getOrNull()
            }
            if (importedSignature == null) tell(text(R.string.msg_image_unreadable))
        }
    }

    // ---- Digital signatures: signing with a digital ID, and what a document's signatures say ----

    /** The digital IDs the reader added or made (lazy: the engine's start may need them first). */
    val digitalIds by lazy { io.github.dikshant363.pdfcraft.engine.DigitalIds(app.filesDir) }

    /** The signers the reader chose to trust, given to the engine when it starts. */
    val trustedCerts by lazy { io.github.dikshant363.pdfcraft.engine.TrustedCertificates(app.filesDir) }

    /** The document a digital signature's box is being drawn on (Sign ▸ Sign with a digital ID). */
    var certPlacing by mutableStateOf<OpenDocument?>(null)

    /** Sign with a digital ID, open for where the signature goes. */
    var certSign by mutableStateOf<io.github.dikshant363.pdfcraft.ui.viewer.CertTarget?>(null)

    /** Why the last signing failed (a wrong password), shown on the Sign screen. */
    var certSignError by mutableStateOf<String?>(null)

    /** The digital ID chosen on the Sign screen. */
    var chosenId by mutableStateOf<io.github.dikshant363.pdfcraft.engine.DigitalId?>(null)

    /** A picked digital ID file, waiting for its password. */
    var idImport by mutableStateOf<io.github.dikshant363.pdfcraft.ui.viewer.IdImport?>(null)
        private set

    /** Create a digital ID is open. */
    var idCreate by mutableStateOf(false)

    /** An empty signature field was tapped: draw a signature there, or sign it with a digital ID. */
    var signFieldChoice by mutableStateOf<Pair<OpenDocument, io.github.dikshant363.pdfcraft.core.WidgetInfo>?>(null)

    /** Draw the box for a new digital signature on [doc], in Fill & Sign. */
    fun startCertPlacing(doc: OpenDocument) {
        if (!doc.info.readOnlyReason.isNullOrEmpty()) {
            tell(text(R.string.msg_sign_read_only))
            return
        }
        if (doc.controller.tool != ViewerTool.FillSign) enterTool(doc, ViewerTool.FillSign)
        doc.controller.signPanel = false
        selectedFill = null
        certPlacing = doc
    }

    /** The box was drawn: on to choosing the ID. */
    fun placeCert(doc: OpenDocument, page: Int, rect: FloatArray) {
        certPlacing = null
        openCertSign(io.github.dikshant363.pdfcraft.ui.viewer.CertTarget(doc, null, page, rect))
    }

    fun signFieldWithId(doc: OpenDocument, w: io.github.dikshant363.pdfcraft.core.WidgetInfo) {
        openCertSign(io.github.dikshant363.pdfcraft.ui.viewer.CertTarget(doc, w.field, w.page.toInt(), null))
    }

    /** A drawn (Fill & Sign) signature in signature field [w], as a tap on it did before digital IDs. */
    fun drawSignatureInField(doc: OpenDocument, w: io.github.dikshant363.pdfcraft.core.WidgetInfo) {
        signaturePad = io.github.dikshant363.pdfcraft.ui.viewer.SignaturePad(false, doc, PagePoint(w.page.toInt(), (w.rect[0] + w.rect[2]) / 2f - 40f, (w.rect[1] + w.rect[3]) / 2f - 10f))
    }

    private fun openCertSign(t: io.github.dikshant363.pdfcraft.ui.viewer.CertTarget) {
        certSignError = null
        val ids = digitalIds.all
        if (chosenId !in ids) chosenId = ids.lastOrNull()
        certSign = t
    }

    fun pickDigitalId() = pickFiles(FilePickKind.DigitalId)

    /** The digital ID file's password: add it (when the password opens it), or give up (null). */
    fun answerIdImport(password: String?) {
        val imp = idImport ?: return
        idImport = null
        if (password == null) {
            imp.file.delete()
            return
        }
        viewModelScope.launch {
            try {
                val info = withContext(Dispatchers.IO) { io.github.dikshant363.pdfcraft.core.digitalIdInfo(imp.file.absolutePath, password) }
                val problem = info.problem
                if (problem != null) {
                    imp.file.delete()
                    tell(text(R.string.msg_id_cannot_sign, problem))
                    return@launch
                }
                val id = withContext(Dispatchers.IO) { digitalIds.add(imp.file, info) }
                imp.file.delete()
                chosenId = id
                tell(text(R.string.msg_id_added, id.name))
            } catch (e: MobileException.WrongPassword) {
                idImport = io.github.dikshant363.pdfcraft.ui.viewer.IdImport(imp.file, imp.name, wrong = true)
            } catch (e: MobileException) {
                imp.file.delete()
                tell(text(R.string.msg_id_unusable, e.userMessage))
            }
        }
    }

    /** Make a self-signed digital ID (2048-bit RSA, valid 5 years) and choose it. */
    fun createDigitalId(name: String, email: String, organization: String, password: String) {
        val eng = engine ?: return
        viewModelScope.launch {
            busy = text(R.string.msg_busy_creating_id)
            val tmp = File(getApplication<Application>().cacheDir, "new-id-${System.nanoTime()}.p12")
            try {
                eng.tool("sign_id_create", mapOf(
                    "name" to name.trim(), "email" to email.trim().ifEmpty { null }, "organization" to organization.trim().ifEmpty { null },
                    "password" to password, "path" to tmp.absolutePath,
                ))
                val info = withContext(Dispatchers.IO) { io.github.dikshant363.pdfcraft.core.digitalIdInfo(tmp.absolutePath, password) }
                chosenId = withContext(Dispatchers.IO) { digitalIds.add(tmp, info) }
                idCreate = false
                tell(text(R.string.msg_id_created))
            } catch (e: MobileException) {
                tell(toolMessage(e, "sign_id_create"))
            } finally {
                tmp.delete()
                busy = null
            }
        }
    }

    fun removeDigitalId(id: io.github.dikshant363.pdfcraft.engine.DigitalId) {
        digitalIds.remove(id)
        if (chosenId == id) chosenId = digitalIds.all.lastOrNull()
    }

    /**
     * Sign [t]'s document with [id]: the engine signs it as it is now (with any changes not yet
     * saved) and the signed file replaces the working copy, then goes back where it came from.
     */
    fun signWithId(
        t: io.github.dikshant363.pdfcraft.ui.viewer.CertTarget,
        id: io.github.dikshant363.pdfcraft.engine.DigitalId,
        password: String,
        look: io.github.dikshant363.pdfcraft.ui.viewer.CertLook,
        reason: String,
        location: String,
    ) {
        val eng = engine ?: return
        val doc = t.doc
        viewModelScope.launch {
            busy = text(R.string.msg_busy_signing)
            try {
                val args = mutableMapOf<String, Any?>(
                    "doc" to doc.id, "id" to id.file.absolutePath, "password" to password, "out" to doc.file.absolutePath,
                    "reason" to reason.trim().ifEmpty { null }, "location" to location.trim().ifEmpty { null },
                )
                if (t.field != null) {
                    args["field"] = t.field
                } else {
                    args["page"] = t.page + 1
                    args["rect"] = t.rect?.map { it.toDouble() }
                }
                when (look) {
                    io.github.dikshant363.pdfcraft.ui.viewer.CertLook.Name -> Unit
                    io.github.dikshant363.pdfcraft.ui.viewer.CertLook.Typed -> args["graphic"] = mapOf("text" to id.name)
                    io.github.dikshant363.pdfcraft.ui.viewer.CertLook.Drawn -> args["graphic"] = mapOf("image" to signatures.signature.absolutePath)
                }
                eng.tool("sign_document", args)
                certSign = null
                certSignError = null
                if (doc.controller.tool == ViewerTool.FillSign) leaveTool(doc)
                refresh(doc)
                // the working copy is signed; the original gets it now, or a copy does
                doc.notWrittenBack = true
                if (save(doc, quiet = true)) {
                    tell(text(R.string.msg_signed_saved))
                } else {
                    tell(text(R.string.msg_signed_not_saved, doc.name), text(R.string.common_save_copy)) { saveCopyAs(doc) }
                }
            } catch (e: MobileException) {
                val m = toolMessage(e, "sign_document")
                certSignError = if (m.contains("password is incorrect", ignoreCase = true)) text(R.string.common_wrong_password) else m
            } finally {
                busy = null
            }
        }
    }

    /** A tool's refusal without the tool's name in front, as a sentence. */
    private fun toolMessage(e: Throwable, tool: String): String =
        e.userMessage.removePrefix("$tool: ").replaceFirstChar { it.uppercase() }.let { if (it.lastOrNull()?.isLetterOrDigit() == true) "$it." else it }

    /** What the document's signatures say (only asked for when it has signatures or signature fields). */
    private suspend fun loadSignatures(eng: Engine, doc: OpenDocument) {
        if (!doc.info.signed && doc.widgets.none { it.kind == "signature" }) {
            doc.signatures = emptyList()
            return
        }
        doc.signatures = runCatching { io.github.dikshant363.pdfcraft.ui.viewer.parseSignatures(eng.tool("sign_list", mapOf("doc" to doc.id))) }.getOrDefault(emptyList())
    }

    /** On opening a signed document: whether its signatures hold, with the details a tap away. */
    private fun announceSignatures(doc: OpenDocument) {
        val signed = doc.signatures.filter { it.signed }
        if (signed.isEmpty()) return
        val one = signed.singleOrNull()
        // the local `text` hides the text() function here: called by its full name
        val text = when {
            signed.any { it.status == "invalid" } -> io.github.dikshant363.pdfcraft.text(if (one != null) R.string.msg_sig_invalid else R.string.msg_sig_some_invalid)
            signed.all { it.status == "valid" } -> if (one != null) {
                one.signer?.let { io.github.dikshant363.pdfcraft.text(R.string.msg_sig_valid, it) } ?: io.github.dikshant363.pdfcraft.text(R.string.msg_sig_valid_unknown)
            } else {
                plural(R.plurals.msg_sig_all_valid, signed.size, signed.size)
            }
            one != null -> one.signer?.let { io.github.dikshant363.pdfcraft.text(R.string.msg_sig_unverified, it) } ?: io.github.dikshant363.pdfcraft.text(R.string.msg_sig_unverified_unknown)
            else -> io.github.dikshant363.pdfcraft.text(R.string.msg_sig_some_unverified)
        }
        tell(text, io.github.dikshant363.pdfcraft.text(R.string.msg_sig_details)) { doc.controller.sheet = io.github.dikshant363.pdfcraft.ui.viewer.ViewerSheet.Signatures }
    }

    /** Show signature [s] on its page, flashing its box. */
    fun showSignature(doc: OpenDocument, s: io.github.dikshant363.pdfcraft.ui.viewer.SignatureEntry) {
        val page = s.page ?: return
        val rect = s.rect ?: return
        doc.viewer.goToPage(page, (rect[1] - 60f).coerceAtLeast(0f))
        viewModelScope.launch {
            delay(250)
            doc.linkFlash = LinkFlash(page, rect)
            delay(600)
            doc.linkFlash = null
        }
    }

    /** Trust the certificate that made signature [s], on this device: signatures by it then validate. */
    fun trustSigner(doc: OpenDocument, s: io.github.dikshant363.pdfcraft.ui.viewer.SignatureEntry) {
        val eng = engine ?: return
        val sha = s.cert?.sha256 ?: return
        viewModelScope.launch {
            try {
                val der = eng.command { signerCertificate(doc.id, s.field) } ?: return@launch
                withContext(Dispatchers.IO) { trustedCerts.add(der, sha) }
                applyTrust(eng)
                loadSignatures(eng, doc)
                tell(s.signer?.let { text(R.string.msg_signer_trusted, it) } ?: text(R.string.msg_signer_trusted_unknown))
            } catch (e: MobileException) {
                tell(e.userMessage)
            }
        }
    }

    fun untrustSigner(doc: OpenDocument, s: io.github.dikshant363.pdfcraft.ui.viewer.SignatureEntry) {
        val eng = engine ?: return
        val sha = s.cert?.sha256 ?: return
        viewModelScope.launch {
            withContext(Dispatchers.IO) { trustedCerts.remove(sha) }
            runCatching { applyTrust(eng) }
            loadSignatures(eng, doc)
        }
    }

    /** Give the engine the trusted signers' certificates (it revalidates open documents). */
    private suspend fun applyTrust(eng: Engine) {
        val paths = withContext(Dispatchers.IO) { trustedCerts.files().map { it.absolutePath } }
        eng.tool("sign_trust", mapOf("clear" to true, "paths" to paths))
    }

    // ---- Edit PDF ----------------------------------------------------------------------------

    var editMode by mutableStateOf(io.github.dikshant363.pdfcraft.ui.viewer.EditMode.Select)

    /** Each page's paragraphs (0-based page), read when Edit PDF shows the page. */
    val paragraphs = androidx.compose.runtime.mutableStateMapOf<Int, List<io.github.dikshant363.pdfcraft.ui.viewer.Paragraph>>()
    private var paragraphsGen = -1L

    /** The paragraph tapped once (outlined), and the one open in the editor. */
    var editSelection by mutableStateOf<io.github.dikshant363.pdfcraft.ui.viewer.Paragraph?>(null)
    var editing by mutableStateOf<io.github.dikshant363.pdfcraft.ui.viewer.Paragraph?>(null)

    suspend fun loadParagraphs(doc: OpenDocument, page: Int) {
        val eng = engine ?: return
        if (paragraphsGen != doc.info.generation.toLong()) {
            paragraphs.clear()
            paragraphsGen = doc.info.generation.toLong()
        }
        if (paragraphs.containsKey(page)) return
        val r = runCatching { eng.tool("text_paragraphs", mapOf("doc" to doc.id, "page" to page + 1)) }.getOrNull() ?: return
        val arr = r.optJSONArray("paragraphs") ?: return
        paragraphs[page] = (0 until arr.length()).mapNotNull { i ->
            val o = arr.optJSONObject(i) ?: return@mapNotNull null
            val rect = o.optJSONArray("rect") ?: return@mapNotNull null
            val font = o.optString("font")
            io.github.dikshant363.pdfcraft.ui.viewer.Paragraph(
                page = page, number = o.optInt("paragraph"), text = o.optString("text"),
                box = FloatArray(4) { k -> rect.optDouble(k).toFloat() }, font = font, size = o.optDouble("size", 12.0).toFloat(),
                bold = if (o.has("bold")) o.optBoolean("bold") else font.contains("Bold", true) || font.contains("Black", true) || font.contains("Heavy", true),
                italic = if (o.has("italic")) o.optBoolean("italic") else font.contains("Italic", true) || font.contains("Oblique", true),
                color = o.optString("color").takeIf { it.startsWith("#") },
            )
        }
        editSelection?.takeIf { it.page == page }?.let { sel -> editSelection = paragraphs[page]?.firstOrNull { it.number == sel.number } }
        editing?.takeIf { it.page == page }?.let { sel -> editing = paragraphs[page]?.firstOrNull { it.number == sel.number } }
    }

    /** A tap in Edit PDF: finish typing, place new text, or select the paragraph or picture under it. */
    fun editTap(doc: OpenDocument, at: PagePoint?) {
        editing?.let { commitInlineEdit(doc) }
        editImage = null
        if (at == null) {
            editSelection = null
            return
        }
        if (editMode == io.github.dikshant363.pdfcraft.ui.viewer.EditMode.AddText) {
            editMode = io.github.dikshant363.pdfcraft.ui.viewer.EditMode.Select
            editSelection = null
            addTextPrompt = TextPrompt(doc, at)
            return
        }
        if (editMode == io.github.dikshant363.pdfcraft.ui.viewer.EditMode.AddImage) {
            editMode = io.github.dikshant363.pdfcraft.ui.viewer.EditMode.Select
            editSelection = null
            addImageAt(doc, at)
            return
        }
        val hit = paragraphs[at.page].orEmpty().firstOrNull { p -> at.x in (p.box[0] - 4)..(p.box[2] + 4) && at.y in (p.box[1] - 4)..(p.box[3] + 4) }
        editSelection = hit
        if (hit == null) selectImageAt(doc, at)
    }

    /** The picture selected in Edit PDF. */
    var editImage by mutableStateOf<io.github.dikshant363.pdfcraft.ui.viewer.EditImage?>(null)

    /** Formatting the engine doesn't report back, kept per paragraph (page to number) while editing. */
    val editFont = androidx.compose.runtime.mutableStateMapOf<Pair<Int, Int>, String>()
    val editAlign = androidx.compose.runtime.mutableStateMapOf<Pair<Int, Int>, String>()
    val editUnderline = androidx.compose.runtime.mutableStateMapOf<Pair<Int, Int>, Boolean>()

    /** Typing in a paragraph: whether it opened with everything selected, and the text so far. */
    var editSelectAll = false
    var editDraft: String? = null

    fun startInlineEdit(para: io.github.dikshant363.pdfcraft.ui.viewer.Paragraph, selectAll: Boolean) {
        editSelectAll = selectAll
        editDraft = para.text
        editSelection = para
        editing = para
    }

    /** Write the paragraph being typed in into the page (when it changed) and stop typing. */
    fun commitInlineEdit(doc: OpenDocument) {
        val para = editing ?: return
        val text = editDraft ?: para.text
        editing = null
        editDraft = null
        if (text != para.text) runTool(doc, "text_edit", mapOf("page" to para.page + 1, "paragraph" to para.number, "text" to text))
    }

    private fun paraArgs(para: io.github.dikshant363.pdfcraft.ui.viewer.Paragraph) = mapOf<String, Any?>("page" to para.page + 1, "paragraph" to para.number)

    /** Make [para] a list of [style] (or plain again), with any typing so far. */
    fun listParagraph(doc: OpenDocument, para: io.github.dikshant363.pdfcraft.ui.viewer.Paragraph, style: io.github.dikshant363.pdfcraft.ui.viewer.ListStyle) {
        val typing = editing != null && editing?.number == para.number && editing?.page == para.page
        val base = if (typing) editDraft ?: para.text else para.text
        val text = io.github.dikshant363.pdfcraft.ui.viewer.ListStyle.apply(base, style)
        if (text == base) return
        if (typing) editDraft = text
        runTool(doc, "text_edit", paraArgs(para) + ("text" to text))
    }

    /** A formatting change to [para], applied to the page at once (with any typing so far). */
    fun styleParagraph(doc: OpenDocument, para: io.github.dikshant363.pdfcraft.ui.viewer.Paragraph, style: Map<String, Any?>) {
        val typed = editDraft?.takeIf { editing != null && it != para.text }
        runTool(doc, "text_edit", paraArgs(para) + style + (if (typed != null) mapOf("text" to typed) else emptyMap()))
    }

    /** Move [para] by ([dx], [dy]) points (right and down on screen). */
    fun moveParagraph(doc: OpenDocument, para: io.github.dikshant363.pdfcraft.ui.viewer.Paragraph, dx: Float, dy: Float) {
        runTool(doc, "text_edit", paraArgs(para) + mapOf("dx" to dx.toDouble(), "dy" to -dy.toDouble()))
    }

    /** Rewrap [para] to [width] points, moving it [shift] points right first (dragged from the left). */
    fun resizeParagraph(doc: OpenDocument, para: io.github.dikshant363.pdfcraft.ui.viewer.Paragraph, width: Float, shift: Float) {
        val args = paraArgs(para) + ("width" to width.toDouble()) + (if (shift != 0f) mapOf("dx" to shift.toDouble()) else emptyMap())
        runTool(doc, "text_edit", args)
    }

    fun deleteParagraph(doc: OpenDocument, para: io.github.dikshant363.pdfcraft.ui.viewer.Paragraph) {
        editSelection = null
        editing = null
        runTool(doc, "text_edit", paraArgs(para) + ("text" to ""))
    }

    /** The page's colour just outside [para], for typing over it. */
    suspend fun paperColor(doc: OpenDocument, para: io.github.dikshant363.pdfcraft.ui.viewer.Paragraph): androidx.compose.ui.graphics.Color? {
        val eng = engine ?: return null
        val x = (para.box[0] - 4f).coerceAtLeast(0f).toInt().toUInt()
        val y = (para.box[1] - 4f).coerceAtLeast(0f).toInt().toUInt()
        val img = runCatching {
            withContext(eng.thumbnails) { eng.session.renderPage(doc.id, para.page.toUInt(), 1f, io.github.dikshant363.pdfcraft.core.TileRect(x, y, 2u, 2u)) }
        }.getOrNull() ?: return null
        val px = img.rgba
        if (px.size < 4) return null
        return androidx.compose.ui.graphics.Color(px[0].toInt() and 0xff, px[1].toInt() and 0xff, px[2].toInt() and 0xff)
    }

    private fun selectImageAt(doc: OpenDocument, at: PagePoint) {
        viewModelScope.launch { imageUnder(doc, at)?.let { editImage = it } }
    }

    /** The picture drawn under [at], topmost first, if any. */
    private suspend fun imageUnder(doc: OpenDocument, at: PagePoint): io.github.dikshant363.pdfcraft.ui.viewer.EditImage? {
        val eng = engine ?: return null
        val r = runCatching { eng.tool("page_images", mapOf("doc" to doc.id, "page" to at.page + 1)) }.getOrNull() ?: return null
        val arr = r.optJSONArray("images") ?: return null
        for (i in arr.length() - 1 downTo 0) {
            val o = arr.optJSONObject(i) ?: continue
            val rect = (o.optJSONArray("visible_rect") ?: o.optJSONArray("rect")) ?: continue
            val box = FloatArray(4) { k -> rect.optDouble(k).toFloat() }
            if (at.x in box[0]..box[2] && at.y in box[1]..box[3]) {
                val full = o.optJSONArray("rect") ?: rect
                return io.github.dikshant363.pdfcraft.ui.viewer.EditImage(at.page, o.optInt("image"), FloatArray(4) { k -> full.optDouble(k).toFloat() })
            }
        }
        return null
    }

    /** The menu a long press on an empty part of a page opens. */
    var pageMenu by mutableStateOf<io.github.dikshant363.pdfcraft.ui.viewer.PageMenu?>(null)

    /**
     * A long press where no word is: on a picture, Edit PDF with the picture selected (the
     * reference's picture menu leads to editing it); elsewhere, the page menu at that point.
     */
    fun longPressPage(doc: OpenDocument, at: PagePoint, screen: androidx.compose.ui.geometry.Offset) {
        val eng = engine ?: return
        viewModelScope.launch {
            val img = imageUnder(doc, at)
            if (img != null) {
                enterTool(doc, io.github.dikshant363.pdfcraft.ui.viewer.ViewerTool.Edit)
                if (doc.controller.tool == io.github.dikshant363.pdfcraft.ui.viewer.ViewerTool.Edit) editImage = img
                return@launch
            }
            val glyphs = runCatching { withContext(eng.commands) { eng.session.glyphCount(doc.id, at.page.toUInt()) } }.getOrDefault(1u)
            pageMenu = io.github.dikshant363.pdfcraft.ui.viewer.PageMenu(doc, at, screen, pageHasText = glyphs > 0u)
        }
    }

    /** Put [img] in [rect] (points): a move or a resize. */
    fun placeImage(doc: OpenDocument, img: io.github.dikshant363.pdfcraft.ui.viewer.EditImage, rect: FloatArray) {
        editImage = img.copy(rect = rect)
        runTool(doc, "image_edit", mapOf("page" to img.page + 1, "image" to img.number, "action" to "move", "rect" to rect.map { it.toDouble() }))
    }

    fun turnImage(doc: OpenDocument, img: io.github.dikshant363.pdfcraft.ui.viewer.EditImage, quarters: Int) {
        runTool(doc, "image_edit", mapOf("page" to img.page + 1, "image" to img.number, "action" to "rotate", "quarters" to quarters)) {
            // the turned picture keeps its centre; read its new box
            selectImageAt(doc, PagePoint(img.page, (img.rect[0] + img.rect[2]) / 2f, (img.rect[1] + img.rect[3]) / 2f))
        }
    }

    fun deleteImage(doc: OpenDocument, img: io.github.dikshant363.pdfcraft.ui.viewer.EditImage) {
        editImage = null
        runTool(doc, "image_edit", mapOf("page" to img.page + 1, "image" to img.number, "action" to "delete"))
    }

    /** New page text being typed in Edit PDF (Add text). */
    var addTextPrompt by mutableStateOf<TextPrompt?>(null)

    fun postAddText(prompt: TextPrompt, text: String) {
        addTextPrompt = null
        if (text.isBlank()) return
        runTool(prompt.doc, "page_add_text", mapOf(
            "page" to prompt.at.page + 1, "text" to text, "at" to listOf(prompt.at.x.toDouble(), prompt.at.y.toDouble()),
            "size" to 12.0, "font" to "helvetica",
        ))
    }

    /** Add image: the activity picks a picture, then [addImage] places it on the current page. */
    val imagePicks = Channel<OpenDocument>(Channel.BUFFERED)
    private var imageFor: OpenDocument? = null

    /** Where the picture being picked goes (centred there); null: the current page as before. */
    private var imageAt: PagePoint? = null

    fun pickEditImage(doc: OpenDocument) {
        imageFor = doc
        imagePicks.trySend(doc)
    }

    /** Add image at a tapped point (Edit PDF's Add image mode, or the page menu): first, from where. */
    fun addImageAt(doc: OpenDocument, at: PagePoint) {
        imageSource = doc to at
    }

    /** Asking where the picture comes from (photos or the camera), as the reference's menu does. */
    var imageSource by mutableStateOf<Pair<OpenDocument, PagePoint>?>(null)

    fun addImageFrom(camera: Boolean) {
        val (doc, at) = imageSource ?: return
        imageSource = null
        imageAt = at
        if (!camera) {
            pickEditImage(doc)
            return
        }
        imageFor = doc
        val dir = File(getApplication<Application>().cacheDir, "camera").apply { mkdirs() }
        val f = File(dir, "image-${System.nanoTime()}.jpg")
        shotFile = f
        imageShot = true
        cameraShots.trySend(f)
    }

    /** The camera's photo is for Add image, not a scan. */
    private var imageShot = false

    fun addImage(uri: Uri?) {
        val doc = imageFor ?: return
        imageFor = null
        if (uri == null) return
        viewModelScope.launch {
            try {
                val name = files.displayName(uri).removeSuffix(".pdf")
                val f = File(getApplication<Application>().cacheDir, "image-${System.nanoTime()}-${name}")
                withContext(Dispatchers.IO) {
                    getApplication<Application>().contentResolver.openInputStream(uri).use { input ->
                        requireNotNull(input) { text(R.string.msg_picture_unreadable) }
                        f.outputStream().use { input.copyTo(it) }
                    }
                }
                if (name.lowercase().endsWith(".gif")) {
                    tell(text(R.string.msg_gif_not_supported))
                    return@launch
                }
                val eng = engine ?: return@launch
                val at = imageAt
                imageAt = null
                val args = mutableMapOf<String, Any?>("doc" to doc.id, "page" to (at?.page ?: doc.viewer.currentPage) + 1, "path" to f.absolutePath)
                // centred on the tapped point, a third of the page wide, kept on the page
                val page = at?.let { doc.info.pages.getOrNull(it.page) }
                val bounds = android.graphics.BitmapFactory.Options().apply { inJustDecodeBounds = true }
                android.graphics.BitmapFactory.decodeFile(f.absolutePath, bounds)
                if (at != null && page != null && bounds.outWidth > 0 && bounds.outHeight > 0) {
                    var w = page.width / 3f
                    var h = w * bounds.outHeight / bounds.outWidth
                    if (h > page.height / 2f) {
                        h = page.height / 2f
                        w = h * bounds.outWidth / bounds.outHeight
                    }
                    val x0 = (at.x - w / 2f).coerceIn(0f, (page.width - w).coerceAtLeast(0f))
                    val y0 = (at.y - h / 2f).coerceIn(0f, (page.height - h).coerceAtLeast(0f))
                    args["rect"] = listOf(x0, y0, x0 + w, y0 + h).map { it.toDouble() }
                }
                eng.tool("page_add_image", args)
                refresh(doc)
            } catch (e: Exception) {
                tell(e.userMessage)
            }
        }
    }

    // ---- Forms -------------------------------------------------------------------------------

    /** The field being filled in. */
    var fieldEdit by mutableStateOf<io.github.dikshant363.pdfcraft.ui.viewer.FieldEdit?>(null)

    /** The top bar's Reset and arrows while a text field is filled. */
    var fieldActions by mutableStateOf<io.github.dikshant363.pdfcraft.ui.viewer.FieldActions?>(null)

    /**
     * A tap at [at]: fill the form field there, if any (a check box or radio button toggles at
     * once). Returns whether there was one.
     */
    fun tapField(doc: OpenDocument, at: PagePoint?): Boolean {
        if (at == null || doc.widgets.isEmpty()) return false
        val w = doc.widgets.firstOrNull { w ->
            w.page.toInt() == at.page && w.rect.size == 4 && at.x in w.rect[0]..w.rect[2] && at.y in w.rect[1]..w.rect[3]
        } ?: return false
        if (w.readOnly) return true
        if (!doc.info.allowsFormFilling) {
            tell(text(R.string.msg_tool_blocked_security))
            return true
        }
        when (w.kind) {
            "checkbox" -> fillField(doc, w.field, !w.on)
            "radio" -> w.export?.let { fillField(doc, w.field, it) }
            "text", "combo", "list" -> fieldEdit = io.github.dikshant363.pdfcraft.ui.viewer.FieldEdit(doc, w)
            // a signed field shows its signature's details; an empty one, a drawn or a digital signature
            "signature" -> if (doc.signatures.any { it.field == w.field && it.signed }) {
                doc.controller.sheet = io.github.dikshant363.pdfcraft.ui.viewer.ViewerSheet.Signatures
            } else {
                signFieldChoice = doc to w
            }
            "button" -> pressButton(doc, w)
            else -> return false
        }
        return true
    }

    /**
     * A form button: does what its release action says, as far as a reader does on a phone:
     * clear the form (or the fields it names), open a web address after asking, go to a page, or
     * run its script. Sending the form online isn't done.
     */
    private fun pressButton(doc: OpenDocument, w: io.github.dikshant363.pdfcraft.core.WidgetInfo) {
        val eng = engine ?: return
        viewModelScope.launch {
            val r = runCatching { eng.tool("form_actions", mapOf("doc" to doc.id, "field" to w.field)) }.getOrNull() ?: return@launch
            val acts = r.optJSONArray("actions") ?: return@launch
            val all = (0 until acts.length()).mapNotNull { acts.optJSONObject(it) }
            val act = all.firstOrNull { it.optString("trigger") == "mouse_up" } ?: all.firstOrNull() ?: return@launch
            when {
                act.has("reset") -> {
                    val names = act.optJSONArray("reset")?.let { a -> (0 until a.length()).map { a.optString(it) } }.orEmpty()
                    runTool(doc, "form_reset", if (names.isEmpty()) emptyMap() else mapOf("fields" to names)) { tell(text(R.string.msg_form_cleared)) }
                }
                act.has("url") -> linkPrompt = act.optString("url")
                act.has("page") -> doc.viewer.goToPage((act.optInt("page") - 1).coerceIn(0, doc.pageCount - 1))
                act.has("submit") -> tell(text(R.string.msg_button_submit))
                act.has("javascript") -> runTool(doc, "js_run", mapOf("field" to w.field, "script" to act.optString("javascript"))) { scriptOutput(doc, it) }
            }
        }
    }

    fun fillField(doc: OpenDocument, field: String, value: Any): Job? {
        val eng = engine ?: return null
        return viewModelScope.launch {
            try {
                val r = eng.tool("form_fill", mapOf("doc" to doc.id, "values" to mapOf(field to value)))
                refresh(doc)
                scriptOutput(doc, r)
            } catch (e: MobileException) {
                // the field refused the value (its format, or its own script's check): said in a
                // dialog, in the form's own words
                showScriptAlert(e.userMessage)
            }
        }
    }

    /** Commits any active form field editor text before closing or navigating away. */
    fun commitFieldEdit(): Job? {
        val edit = fieldEdit ?: return null
        fieldEdit = null
        return edit.commit(this)
    }

    /** What a form's scripts said (their alerts), shown one at a time. */
    val scriptAlerts = androidx.compose.runtime.mutableStateListOf<String>()

    private fun showScriptAlert(message: String) {
        // a script that alerts in a loop can't queue dialogs without end
        if (message.isNotBlank() && scriptAlerts.size < MAX_SCRIPT_ALERTS) scriptAlerts.add(message)
    }

    fun dismissScriptAlert() {
        if (scriptAlerts.isNotEmpty()) scriptAlerts.removeAt(0)
    }

    /**
     * What a form's scripts said and asked of the viewer, from a tool's result: alerts are shown, a
     * page is shown, a web address asks first and printing prints; sending the form online isn't done.
     */
    private fun scriptOutput(doc: OpenDocument, r: JSONObject) {
        // a script that failed: kept out of sight, as the reference keeps it, but in the log
        r.optString("error").takeIf { it.isNotEmpty() && it != "null" }?.let { android.util.Log.w("PdfCraft", "form script: $it") }
        r.optJSONArray("alerts")?.let { a -> for (i in 0 until a.length()) showScriptAlert(a.optString(i)) }
        val requests = r.optJSONArray("requests") ?: return
        for (i in 0 until requests.length()) {
            val q = requests.optJSONObject(i) ?: continue
            when {
                q.has("page") -> doc.viewer.goToPage((q.optInt("page") - 1).coerceIn(0, doc.pageCount - 1))
                q.has("url") -> linkPrompt = q.optString("url")
                q.has("print") -> print(doc)
                q.has("submit") -> tell(text(R.string.msg_button_submit))
            }
        }
    }

    /** Move to the next (or previous) fillable text or choice field in reading order. */
    fun nextField(doc: OpenDocument, from: io.github.dikshant363.pdfcraft.core.WidgetInfo, forward: Boolean) {
        val fillable = doc.widgets.filter { !it.readOnly && (it.kind == "text" || it.kind == "combo" || it.kind == "list") }
        val i = fillable.indexOfFirst { it.field == from.field && it.page == from.page && it.rect == from.rect }
        val next = fillable.getOrNull(if (forward) i + 1 else i - 1)
        fieldEdit = next?.let {
            if (it.page.toInt() != doc.viewer.currentPage) doc.viewer.goToPage(it.page.toInt(), (it.rect[1] - 60f).coerceAtLeast(0f))
            io.github.dikshant363.pdfcraft.ui.viewer.FieldEdit(doc, it)
        }
    }

    // ---- Tool jobs: each runs on the phone and saves its result into Downloads ---------------

    /** File pickers for the activity: what the files are for, the types, and whether several may be picked. */
    val filePicks = Channel<FilePickKind>(Channel.BUFFERED)

    /** Photos for Scan: the activity has the camera app write each into this file. */
    val cameraShots = Channel<File>(Channel.BUFFERED)

    /** The files gathered for Combine, in order. */
    val combineFiles = androidx.compose.runtime.mutableStateListOf<PickedFile>()

    /** The pages scanned so far. */
    val scanPages = androidx.compose.runtime.mutableStateListOf<ScanPage>()

    /** The scan's file name (without .pdf), set when its first page is taken and changeable before saving. */
    var scanName by mutableStateOf("")
    private var shotFile: File? = null
    private var retakeIndex: Int? = null

    // A scan Android closed the app in the middle of, while the camera app was in front, carries on
    // when the photo comes back (#38): what it needs is kept in the saved state, read when it's saved.
    private var scanRestoring: String? = null
    private val scanRestore: kotlinx.coroutines.Job? = saved.get<android.os.Bundle>(SCAN_STATE)?.getString(SCAN_STATE)?.let { text ->
        ScanSession.decode(text)?.let { restoreScan(text, it) }
    }

    init {
        saved.setSavedStateProvider(SCAN_STATE) {
            android.os.Bundle().apply { putString(SCAN_STATE, scanRestoring ?: scanSession()?.encode()) }
        }
    }

    /** The scan in progress (or the Add image photo being taken); null when there is none. */
    private fun scanSession(): ScanSession? {
        if (shotFile == null && scanPages.isEmpty()) return null
        return ScanSession(shotFile?.path, retakeIndex, imageShot, scanName, scanPages.map { ScanSession.Page(it.file.path, it.corners, it.found) })
    }

    private fun restoreScan(text: String, session: ScanSession): kotlinx.coroutines.Job {
        scanRestoring = text
        shotFile = session.shot?.let(::File)
        retakeIndex = session.retake
        imageShot = session.forImage
        scanName = session.name
        return viewModelScope.launch {
            val pages = withContext(Dispatchers.IO) {
                session.pages.mapNotNull { p -> File(p.path).let { f -> scanThumb(f)?.let { ScanPage(f, it, p.corners, p.found) } } }
            }
            scanPages.addAll(pages)
            scanRestoring = null
            // back on the Scan screen with its pages (with none, it would open the camera itself)
            if (pages.isNotEmpty() && toolScreen == null) toolScreen = ToolScreen.Scan
        }
    }

    /** A scanned photo, small and upright, for the Scan screen; null when it can't be read. */
    private fun scanThumb(f: File): androidx.compose.ui.graphics.ImageBitmap? {
        val opts = android.graphics.BitmapFactory.Options().apply { inSampleSize = 4 }
        return android.graphics.BitmapFactory.decodeFile(f.absolutePath, opts)?.let { rotateForExif(f, it) }?.asImageBitmap()
    }

    /** Scan's pages go into this document at this (1-based) position instead of a new PDF. */
    var insertScanAt: Pair<OpenDocument, Int>? = null
    private var insertFileAt: Pair<OpenDocument, Int>? = null

    fun pickFiles(kind: FilePickKind) {
        filePicks.trySend(kind)
    }

    /** Files picked for [kind]. */
    fun onFilesPicked(kind: FilePickKind, uris: List<Uri>) {
        if (uris.isEmpty()) return
        when (kind) {
            FilePickKind.Combine -> viewModelScope.launch {
                val picked = withContext(Dispatchers.IO) { uris.map { u -> PickedFile(u, files.displayNameRaw(u), files.size(u)) } }
                combineFiles.addAll(picked.filter { p -> combineFiles.none { it.uri == p.uri } })
                if (toolScreen !is ToolScreen.Combine) toolScreen = ToolScreen.Combine(null)
            }
            FilePickKind.Create -> createFrom(uris)
            FilePickKind.Insert -> {
                val (doc, at) = insertFileAt ?: return
                insertFileAt = null
                viewModelScope.launch {
                    val f = withContext(Dispatchers.IO) { copyIn(uris.first()) }
                    runTool(doc, "page_insert_file", mapOf("path" to f.absolutePath, "at" to at))
                }
            }
            FilePickKind.Open -> open(uris.first())
            FilePickKind.DigitalId -> viewModelScope.launch {
                val uri = uris.first()
                val picked = withContext(Dispatchers.IO) { runCatching { copyIn(uri) }.getOrNull() }
                if (picked == null) tell(text(R.string.msg_that_file_unreadable))
                else idImport = io.github.dikshant363.pdfcraft.ui.viewer.IdImport(picked, picked.name, wrong = false)
            }
        }
    }

    fun pickInsertFile(doc: OpenDocument, at: Int) {
        insertFileAt = doc to at
        pickFiles(FilePickKind.Insert)
    }

    /** A copy of a picked file in the app's cache, under its own name. */
    private fun copyIn(uri: Uri): File {
        val dir = File(getApplication<Application>().cacheDir, "picked/${System.nanoTime()}").apply { mkdirs() }
        val out = File(dir, files.displayNameRaw(uri))
        getApplication<Application>().contentResolver.openInputStream(uri).use { input ->
            requireNotNull(input) { text(R.string.msg_named_file_unreadable, out.name) }
            out.outputStream().use { input.copyTo(it) }
        }
        return out
    }

    /** Run [block] with the busy dialog showing [message]; failures are told in a snackbar. */
    private fun job(message: String, block: suspend (Engine) -> Unit) {
        val eng = engine ?: return
        viewModelScope.launch {
            busy = message
            try {
                block(eng)
            } catch (e: Exception) {
                tell(e.userMessage)
            } finally {
                busy = null
            }
        }
    }

    /** Put [file] into Downloads as [name] and say so, with Open. */
    private suspend fun deliver(file: File, name: String, mime: String = "application/pdf") {
        val uri = withContext(Dispatchers.IO) { files.saveToDownloads(file, name, mime).also { file.delete() } }
        val size = withContext(Dispatchers.IO) { files.size(uri) }
        val saved = size?.let { text(R.string.msg_saved_downloads_size, name, io.github.dikshant363.pdfcraft.ui.viewer.formatSize(it)) } ?: text(R.string.msg_saved_downloads, name)
        if (mime == "application/pdf") tell(saved, text(R.string.common_open)) { open(uri) }
        else tell(saved, text(R.string.common_open)) {
            intents.trySend(Intent.createChooser(Intent(Intent.ACTION_VIEW).setDataAndType(uri, mime).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION), null))
        }
    }

    private fun tempFile(ext: String) = File(getApplication<Application>().cacheDir, "out-${System.nanoTime()}.$ext")

    /** Compress PDF at [level] (0 high, 1 medium, 2 low compression). */
    fun compress(doc: OpenDocument, level: Int) = job(text(R.string.msg_busy_compressing)) { eng ->
        val (ppi, above, quality) = when (level) {
            0 -> Triple(72, 108, 40)
            2 -> Triple(300, 450, 80)
            else -> Triple(150, 225, 60)
        }
        val images = mapOf("downsample" to true, "ppi" to ppi, "above_ppi" to above, "compression" to "jpeg", "quality" to quality)
        val out = tempFile("pdf")
        eng.tool("doc_optimize", mapOf(
            "doc" to doc.id, "path" to out.absolutePath, "color" to images, "gray" to images,
            "discard_thumbnails" to true, "remove_unreferenced_dests" to true,
        ))
        toolScreen = null
        if (out.length() >= doc.info.fileSize.toLong()) {
            // as the reference: nothing is saved when the file can't get any smaller
            out.delete()
            tell(text(R.string.msg_compress_no_gain))
            return@job
        }
        deliver(out, "${doc.title}_compressed.pdf")
    }

    /** Combine the gathered files, in order, into [name].pdf. */
    fun combine(name: String) = job(text(R.string.msg_busy_combining)) { eng ->
        val paths = withContext(Dispatchers.IO) { combineFiles.map { copyIn(it.uri).absolutePath } }
        val out = tempFile("pdf")
        eng.tool("doc_combine", mapOf("paths" to paths, "out" to out.absolutePath))
        toolScreen = null
        combineFiles.clear()
        deliver(out, DocumentFiles.safeName(name))
    }

    /**
     * A protected copy of [doc] with [password] as its open password. The copy is made from a
     * second, temporary engine document, so the open one keeps its file and history.
     */
    fun protect(doc: OpenDocument, password: String) = job(text(R.string.msg_busy_setting_password)) { eng ->
        save(doc)
        val out = tempFile("pdf")
        val copy = eng.tool("doc_open", mapOf("path" to doc.file.absolutePath, "password" to doc.password)).optLong("doc")
        try {
            eng.tool("doc_protect", mapOf("doc" to copy, "open_password" to password))
            eng.tool("doc_save", mapOf("doc" to copy, "path" to out.absolutePath, "full" to true))
        } finally {
            runCatching { eng.tool("doc_close", mapOf("doc" to copy, "discard_changes" to true)) }
        }
        toolScreen = null
        deliver(out, "${doc.title}_protected.pdf")
    }

    /** Crop [page] (or every page) to [rect] (points from the page's top-left). */
    fun crop(doc: OpenDocument, page: Int?, rect: FloatArray) {
        val args = mutableMapOf<String, Any?>("rect" to rect.map { it.toDouble() })
        if (page != null) args["pages"] = listOf(page + 1)
        runTool(doc, "page_set_box", args) {
            toolScreen = null
            tell(text(if (page == null) R.string.msg_crop_all else R.string.msg_crop_page))
        }
    }

    /** Export [doc] as a Word, RTF, Excel or PowerPoint document. */
    fun exportDocument(doc: OpenDocument, ext: String) = job(text(R.string.msg_busy_exporting_to, ext.uppercase())) { eng ->
        val out = tempFile(ext)
        eng.tool("doc_export_office", mapOf("doc" to doc.id, "path" to out.absolutePath))
        toolScreen = null
        val mime = when (ext) {
            "docx" -> "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            "rtf" -> "application/rtf"
            "xlsx" -> "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            "pptx" -> "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            else -> "text/html"
        }
        deliver(out, "${doc.title}.$ext", mime)
    }

    /** Every page of [doc] as an image (200 dpi) in Pictures/PdfCraft. */
    /** Export to images: the format chosen, while its pages are being picked in the page grid. */
    var imageExport by mutableStateOf<String?>(null)

    /** The image format last exported to, offered first next time (as the reference remembers it). */
    val lastImageFormat: String get() = appPrefs.getString("imageFormat", "jpg") ?: "jpg"

    /** Export to images: remember the format, then pick the pages in the page grid (Save or Share there). */
    fun exportImages(doc: OpenDocument, ext: String) {
        appPrefs.edit().putString("imageFormat", ext).apply()
        toolScreen = null
        imageExport = ext
        organizing = true
        doc.controller.pagesOpen = true
    }

    /** Pictures of [pages] (0-based) at 200 dpi, saved to Pictures/PdfCraft or handed to the share sheet. */
    fun writeImages(doc: OpenDocument, ext: String, pages: List<Int>, share: Boolean) = job(text(R.string.msg_busy_exporting_images)) { eng ->
        val mime = if (ext == "png") "image/png" else "image/jpeg"
        val made = mutableListOf<Pair<File, String>>()
        for ((n, i) in pages.withIndex()) {
            busy = text(R.string.msg_busy_exporting_page, n + 1, pages.size)
            val img = withContext(eng.renders) { eng.session.renderPage(doc.id, i.toUInt(), 200f / 72f, null) }
            val out = tempFile(ext)
            withContext(Dispatchers.IO) {
                val bmp = img.toBitmap()
                // JPEG has no transparency: put the page on white
                val flat = if (ext == "png") bmp else android.graphics.Bitmap.createBitmap(bmp.width, bmp.height, android.graphics.Bitmap.Config.ARGB_8888).also {
                    val c = android.graphics.Canvas(it)
                    c.drawColor(android.graphics.Color.WHITE)
                    c.drawBitmap(bmp, 0f, 0f, null)
                }
                out.outputStream().use { flat.compress(if (ext == "png") android.graphics.Bitmap.CompressFormat.PNG else android.graphics.Bitmap.CompressFormat.JPEG, 90, it) }
            }
            made += out to "${doc.title}_${i + 1}.$ext"
        }
        imageExport = null
        organizing = false
        doc.controller.pagesOpen = false
        if (share) {
            val uris = try {
                withContext(Dispatchers.IO) { files.shareableAll(made) }
            } finally {
                made.forEach { it.first.delete() }
            }
            intents.trySend(Intent.createChooser(sendIntent(uris, mime), null))
        } else {
            withContext(Dispatchers.IO) {
                for ((f, name) in made) {
                    files.saveToPictures(f, name, mime)
                    f.delete()
                }
            }
            tell(plural(R.plurals.msg_images_saved, made.size, made.size))
        }
    }

    /** Create PDF from picked files (images, text, Office documents, PDFs), combined in order, then open it. */
    private fun createFrom(picked: List<Uri>) = job(text(R.string.msg_busy_creating_pdf)) { eng ->
        // at most 12 files at once, as the reference allows
        if (picked.size > MAX_CREATE_FILES) tell(plural(R.plurals.msg_create_first_files, MAX_CREATE_FILES, MAX_CREATE_FILES))
        val uris = picked.take(MAX_CREATE_FILES)
        val paths = withContext(Dispatchers.IO) { uris.map { copyIn(it).absolutePath } }
        val out = tempFile("pdf")
        eng.tool("doc_create_multiple", mapOf("paths" to paths, "out" to out.absolutePath, "mode" to "combine"))
        val name = files.displayNameRaw(uris.first()).substringBeforeLast('.') + ".pdf"
        val uri = withContext(Dispatchers.IO) { files.saveToDownloads(out, DocumentFiles.safeName(name)).also { out.delete() } }
        tell(text(R.string.msg_saved_downloads, name))
        open(uri)
    }

    /** Pages [pages] (0-based) of [doc] as a new PDF in Downloads. */
    fun extractPages(doc: OpenDocument, pages: List<Int>) = job(text(R.string.msg_busy_extracting)) { eng ->
        val out = tempFile("pdf")
        eng.tool("page_extract", mapOf("doc" to doc.id, "pages" to pages.map { it + 1 }, "out" to out.absolutePath))
        deliver(out, "Pages from ${doc.title}.pdf")
    }

    // ---- Scan ---------------------------------------------------------------------------------

    fun takeScanPhoto(retake: Int? = null) {
        val dir = File(getApplication<Application>().cacheDir, "camera").apply { mkdirs() }
        val f = File(dir, "scan-${System.nanoTime()}.jpg")
        shotFile = f
        retakeIndex = retake
        cameraShots.trySend(f)
    }

    fun retakeScan(index: Int) = takeScanPhoto(index)

    /** The camera app came back: [ok] when it took the photo. */
    fun onScanPhoto(ok: Boolean) {
        val f = shotFile ?: return
        shotFile = null
        if (imageShot) {
            imageShot = false
            // after Android closed the app, the document the picture was for isn't open any more
            if (!ok || !f.isFile || f.length() == 0L || imageFor == null) {
                f.delete()
                imageFor = null
                imageAt = null
                return
            }
            // turned upright as the camera meant it (the page keeps no orientation tag), then placed
            viewModelScope.launch {
                val upright = withContext(Dispatchers.IO) {
                    runCatching {
                        val bmp = android.graphics.BitmapFactory.decodeFile(f.absolutePath)?.let { rotateForExif(f, it) } ?: return@runCatching f
                        val out = File(f.parentFile, "upright-${f.name}")
                        out.outputStream().use { bmp.compress(android.graphics.Bitmap.CompressFormat.JPEG, 90, it) }
                        out
                    }.getOrDefault(f)
                }
                addImage(Uri.fromFile(upright))
            }
            return
        }
        if (!ok || !f.isFile || f.length() == 0L) {
            viewModelScope.launch {
                scanRestore?.join()
                if (scanPages.isEmpty()) toolScreen = null
            }
            return
        }
        viewModelScope.launch {
            // the pages taken before Android closed the app come first
            scanRestore?.join()
            val thumb = withContext(Dispatchers.IO) { scanThumb(f) } ?: return@launch
            val page = ScanPage(f, thumb)
            val r = retakeIndex
            retakeIndex = null
            if (r != null && r in scanPages.indices) scanPages[r] = page else scanPages.add(page)
            if (scanName.isBlank()) scanName = "Scan " + java.text.SimpleDateFormat("yyyy-MM-dd HH.mm.ss", java.util.Locale.US).format(java.util.Date())
            if (toolScreen != ToolScreen.Scan) toolScreen = ToolScreen.Scan
            // the document's edges in the photo, shown on the page and moved in Crop; the whole
            // photo is kept when none are found
            // (the engine may still be starting when the app was closed and opened again)
            val found = (engine ?: application.engine.await().getOrNull())?.let { eng -> runCatching { eng.tool("photo_find_document", mapOf("path" to f.absolutePath)) }.getOrNull() }
                ?.takeIf { it.optBoolean("found") }?.optJSONArray("corners")?.takeIf { it.length() == 8 }
                ?.let { a -> FloatArray(8) { a.optDouble(it).toFloat() } }
            val at = scanPages.indexOfFirst { it.file == f }
            if (found != null && at >= 0) scanPages[at] = scanPages[at].copy(corners = found, found = found)
        }
    }

    private fun rotateForExif(f: File, bmp: android.graphics.Bitmap): android.graphics.Bitmap {
        val deg = runCatching {
            when (androidx.exifinterface.media.ExifInterface(f.absolutePath).getAttributeInt(androidx.exifinterface.media.ExifInterface.TAG_ORIENTATION, 1)) {
                androidx.exifinterface.media.ExifInterface.ORIENTATION_ROTATE_90 -> 90f
                androidx.exifinterface.media.ExifInterface.ORIENTATION_ROTATE_180 -> 180f
                androidx.exifinterface.media.ExifInterface.ORIENTATION_ROTATE_270 -> 270f
                else -> 0f
            }
        }.getOrDefault(0f)
        if (deg == 0f) return bmp
        val m = android.graphics.Matrix().apply { postRotate(deg) }
        return android.graphics.Bitmap.createBitmap(bmp, 0, 0, bmp.width, bmp.height, m, true)
    }

    fun discardScan() {
        scanPages.forEach { it.file.delete() }
        scanPages.clear()
        scanName = ""
        insertScanAt = null
        toolScreen = null
    }

    /** Save the scanned pages as a PDF with [filter]'s look (or insert them where Organize asked). */
    fun saveScan(filter: ScanFilter) = job(text(R.string.msg_busy_saving_scan)) { eng ->
        val pages = scanPages.toList()
        val paths = pages.map { it.file.absolutePath }
        val created = eng.tool("doc_create", mapOf("from" to "images", "paths" to paths, "name" to "Scan"))
        val id = created.optLong("doc")
        try {
            // each page keeps the part inside its corners, straightened, with the look; a whole
            // photo in Original stays as it was taken
            val sizes = eng.tool("doc_info", mapOf("doc" to id)).optJSONArray("pages")
            for ((i, page) in pages.withIndex()) {
                if (page.corners.contentEquals(ScanPage.WHOLE) && filter == ScanFilter.Original) continue
                val size = sizes?.optJSONObject(i) ?: continue
                val (w, h) = size.optDouble("width") to size.optDouble("height")
                val corners = page.corners.mapIndexed { k, v -> v * if (k % 2 == 0) w else h }
                busy = text(R.string.msg_busy_cleaning_page, i + 1, pages.size)
                runCatching { eng.tool("page_enhance_camera", mapOf("doc" to id, "page" to i + 1, "corners" to corners, "mode" to filter.mode, "level" to filter.level)) }
            }
            // the scan's words become text, searchable and selectable, when text recognition is installed
            withContext(Dispatchers.IO) { runCatching { eng.prepareOcrModels(getApplication()) } }
            if (runCatching { eng.tool("ocr_status") }.getOrNull()?.optBoolean("available", false) == true) {
                busy = text(R.string.msg_busy_recognizing)
                runCatching { eng.tool("ocr_recognize", mapOf("doc" to id)) }
            }
            val out = tempFile("pdf")
            eng.tool("doc_save", mapOf("doc" to id, "path" to out.absolutePath, "full" to true))
            val target = insertScanAt
            insertScanAt = null
            val chosen = scanName.trim()
            scanPages.forEach { it.file.delete() }
            scanPages.clear()
            scanName = ""
            toolScreen = null
            if (target != null) {
                runTool(target.first, "page_insert_file", mapOf("path" to out.absolutePath, "at" to target.second))
            } else {
                val stamp = java.text.SimpleDateFormat("yyyy-MM-dd HH.mm.ss", java.util.Locale.US).format(java.util.Date())
                val name = DocumentFiles.safeName(chosen.ifEmpty { "Scan $stamp" })
                val uri = withContext(Dispatchers.IO) { files.saveToDownloads(out, name).also { out.delete() } }
                tell(text(R.string.msg_saved_downloads, name))
                open(uri)
            }
        } finally {
            runCatching { eng.tool("doc_close", mapOf("doc" to id, "discard_changes" to true)) }
        }
    }

    // ---- Home, Files, Search -----------------------------------------------------------------

    var homeTab by mutableStateOf(io.github.dikshant363.pdfcraft.ui.home.HomeTab.Home)
    var settingsOpen by mutableStateOf(false)

    /** The file whose "..." sheet is open, and whether it was opened from Recent. */
    var fileSheet by mutableStateOf<Pair<io.github.dikshant363.pdfcraft.ui.home.FileRow, Boolean>?>(null)
    var showLicences by mutableStateOf(false)
    var filesShowLocal by mutableStateOf(false)

    /** Lists as a grid instead of rows (remembered). */
    private var gridViewState by mutableStateOf(appPrefs.getBoolean("gridView", false))
    var gridView: Boolean
        get() = gridViewState
        set(v) {
            gridViewState = v
            appPrefs.edit().putBoolean("gridView", v).apply()
        }

    /** Starred: 0 by date, 1 by name. */
    private var starSortState by mutableStateOf(appPrefs.getInt("starSort", 0))
    var starSort: Int
        get() = starSortState
        set(v) {
            starSortState = v
            appPrefs.edit().putInt("starSort", v).apply()
        }

    /** Bumped when a preference shown on screen changes outside Compose state. */
    var prefsChanged by mutableStateOf(0)

    val thumbnails = io.github.dikshant363.pdfcraft.engine.Thumbnails(app)

    var localFiles by mutableStateOf<List<io.github.dikshant363.pdfcraft.engine.LocalPdf>>(emptyList())
        private set
    var scanning by mutableStateOf(false)
        private set
    var hasFileAccess by mutableStateOf(io.github.dikshant363.pdfcraft.engine.LocalFiles.hasAccess(app))
        private set
    private var lastScan = 0L

    /** The app's version, as the About page shows it. */
    val appVersion: String = runCatching { app.packageManager.getPackageInfo(app.packageName, 0).versionName }.getOrNull() ?: "?"

    /** Look for the PDFs on the phone (at most every half minute unless [force]d). */
    fun scanLocal(force: Boolean = false) {
        hasFileAccess = io.github.dikshant363.pdfcraft.engine.LocalFiles.hasAccess(getApplication())
        val now = System.currentTimeMillis()
        if (!force && now - lastScan < 30_000 && localFiles.isNotEmpty()) return
        lastScan = now
        viewModelScope.launch {
            scanning = true
            localFiles = withContext(Dispatchers.IO) { io.github.dikshant363.pdfcraft.engine.LocalFiles.scan(getApplication()) }
            scanning = false
        }
    }

    /** Adding a folder to On this device: the activity opens the system's folder picker. */
    val accessRequests = Channel<Unit>(Channel.CONFLATED)

    fun requestFileAccess() {
        accessRequests.trySend(Unit)
    }

    /** The folders On this device lists, with their names. */
    fun localFolders(): List<Pair<Uri, String>> {
        val app = getApplication<Application>()
        return io.github.dikshant363.pdfcraft.engine.LocalFiles.folders(app).map { it to io.github.dikshant363.pdfcraft.engine.LocalFiles.folderName(app, it) }
    }

    /** A folder chosen in the picker: list its PDFs from now on. */
    fun addLocalFolder(tree: Uri) {
        io.github.dikshant363.pdfcraft.engine.LocalFiles.addFolder(getApplication(), tree)
        hasFileAccess = io.github.dikshant363.pdfcraft.engine.LocalFiles.hasAccess(getApplication())
        scanLocal(force = true)
    }

    /** Stop listing [tree]. */
    fun removeLocalFolder(tree: Uri) {
        io.github.dikshant363.pdfcraft.engine.LocalFiles.removeFolder(getApplication(), tree)
        hasFileAccess = io.github.dikshant363.pdfcraft.engine.LocalFiles.hasAccess(getApplication())
        scanLocal(force = true)
    }

    /** Back from the access page (or the app resumed): look again. */
    fun onResumed() {
        val had = hasFileAccess
        hasFileAccess = io.github.dikshant363.pdfcraft.engine.LocalFiles.hasAccess(getApplication())
        if (hasFileAccess && !had) scanLocal(force = true)
        refreshRecents()
    }

    /** What [uri]'s thumbnail is drawn from: the file itself, the app's working copy, or the document. */
    private fun thumbPathFor(uri: String): String? {
        val u = Uri.parse(uri)
        return when (u.scheme) {
            "file" -> u.path
            else -> files.existingCopy(u)?.absolutePath ?: uri
        }
    }

    fun recentRows(): List<io.github.dikshant363.pdfcraft.ui.home.FileRow> = recentList.map { r ->
        io.github.dikshant363.pdfcraft.ui.home.FileRow(r.uri, r.name, thumbPathFor(r.uri), locationOf(r.uri), r.opened, r.size, r.starred) { open(Uri.parse(r.uri)) }
    }

    fun starredRows(): List<io.github.dikshant363.pdfcraft.ui.home.FileRow> = recentRows().filter { it.starred }.let { l ->
        if (starSort == 1) l.sortedBy { it.name.lowercase() } else l.sortedByDescending { it.date }
    }

    fun localRows(): List<io.github.dikshant363.pdfcraft.ui.home.FileRow> {
        // a file opened from another app is known by a different address than the scan's path,
        // so stars are matched by the file itself (name and size)
        val stars = recentList.filter { it.starred }.flatMap { listOf(it.uri, sameFile(it.name, it.size)) }.toSet()
        return localFiles.map { f ->
            val key = f.uri.toString()
            io.github.dikshant363.pdfcraft.ui.home.FileRow(key, f.name, f.path ?: key, f.folder, f.modified, f.size, key in stars || sameFile(f.name, f.size) in stars) { open(f.uri) }
        }
    }

    private fun sameFile(name: String, size: Long) = "$name|$size"

    /** Files whose names contain [query]: the phone's and Recent's, each file once. */
    fun searchFiles(query: String): List<io.github.dikshant363.pdfcraft.ui.home.FileRow> {
        val q = query.trim().lowercase()
        val all = localRows() + recentRows()
        return all.filter { q in it.name.lowercase() }.distinctBy { it.key }.distinctBy { sameFile(it.name, it.size) }
    }

    private fun locationOf(uri: String): String = runCatching { files.displayLocation(Uri.parse(uri)) }.getOrDefault(text(R.string.files_location_this_device))

    fun clearRecents() {
        viewModelScope.launch {
            withContext(Dispatchers.IO) {
                // starred files stay: the stars live with them
                val keep = recents.list().filter { it.starred }
                recents.clear()
                keep.reversed().forEach { recents.touch(it) }
            }
            refreshRecents()
        }
    }

    fun removeRecent(key: String) {
        viewModelScope.launch {
            withContext(Dispatchers.IO) { recents.remove(setOf(key)) }
            refreshRecents()
        }
    }

    /** What to do with the next PDF opened from a tool tile (Home, Tools). */
    private var pendingTool: io.github.dikshant363.pdfcraft.ui.home.FileTool? = null

    /** A tool that starts by choosing a PDF: pick one, open it, then start the tool. */
    fun toolForFile(tool: io.github.dikshant363.pdfcraft.ui.home.FileTool) {
        pendingTool = tool
        pickFiles(FilePickKind.Open)
    }

    /** Open [row] and start [tool] on it. */
    fun openFor(row: io.github.dikshant363.pdfcraft.ui.home.FileRow, tool: io.github.dikshant363.pdfcraft.ui.home.FileTool) {
        pendingTool = tool
        row.open()
    }

    /** A document just opened: start the tool it was opened for, if any. */
    private fun startPendingTool(doc: OpenDocument) {
        val t = pendingTool ?: return
        pendingTool = null
        when (t) {
            FileTool.Edit -> enterTool(doc, ViewerTool.Edit)
            FileTool.ReadAloud -> startReadAloud(doc)
            FileTool.FillSign -> enterTool(doc, ViewerTool.FillSign)
            FileTool.Comment -> enterTool(doc, ViewerTool.Comment)
            FileTool.Organize -> { organizing = true; doc.controller.pagesOpen = true }
            FileTool.Compress -> openCompress(doc)
            FileTool.Export -> openExport(doc)
            FileTool.Protect -> openProtect(doc)
            FileTool.Crop -> openCrop(doc)
            FileTool.Ocr -> recognizeText(doc)
        }
    }

    /** A file row's local file, for sharing and the like: the file itself, or a fresh copy. */
    private suspend fun localFileOf(row: io.github.dikshant363.pdfcraft.ui.home.FileRow): File? = withContext(Dispatchers.IO) {
        val u = Uri.parse(row.key)
        runCatching { if (u.scheme == "file") File(requireNotNull(u.path)) else files.workingCopy(u) }.getOrNull()
    }

    /** Files ticked in Home's lists (a long press starts it), keyed by where they are. */
    val selectedFiles = androidx.compose.runtime.mutableStateMapOf<String, io.github.dikshant363.pdfcraft.ui.home.FileRow>()

    fun toggleSelected(row: io.github.dikshant363.pdfcraft.ui.home.FileRow) {
        if (selectedFiles.remove(row.key) == null) selectedFiles[row.key] = row
    }

    /** Several PDFs to the share sheet at once. */
    fun shareFiles(rows: List<io.github.dikshant363.pdfcraft.ui.home.FileRow>) {
        handOut {
            val local = rows.mapNotNull { r -> localFileOf(r)?.let { it to r.name } }
            if (local.isEmpty()) error(text(R.string.msg_files_unreadable))
            sendIntent(withContext(Dispatchers.IO) { files.shareableAll(local) })
        }
    }

    /** Combine the ticked files, in the order shown. */
    fun combineSelected(rows: List<io.github.dikshant363.pdfcraft.ui.home.FileRow>) {
        combineFiles.clear()
        rows.take(12).forEach { combineFiles.add(PickedFile(Uri.parse(it.key), it.name, it.size)) }
        toolScreen = ToolScreen.Combine(null)
    }

    fun removeRecents(rows: List<io.github.dikshant363.pdfcraft.ui.home.FileRow>) {
        viewModelScope.launch {
            withContext(Dispatchers.IO) { recents.remove(rows.map { it.key }.toSet()) }
            refreshRecents()
        }
    }

    fun shareFile(row: io.github.dikshant363.pdfcraft.ui.home.FileRow) {
        handOut {
            val f = localFileOf(row) ?: error(text(R.string.common_file_unreadable))
            sendIntent(listOf(withContext(Dispatchers.IO) { files.shareable(f, row.name) }))
        }
    }

    fun printFile(row: io.github.dikshant363.pdfcraft.ui.home.FileRow) {
        viewModelScope.launch {
            val f = localFileOf(row) ?: return@launch tell(text(R.string.common_file_unreadable))
            prints.trySend(f to row.name)
        }
    }

    fun openWith(row: io.github.dikshant363.pdfcraft.ui.home.FileRow) {
        handOut {
            val f = localFileOf(row) ?: error(text(R.string.common_file_unreadable))
            val uri = withContext(Dispatchers.IO) { files.shareable(f, row.name) }
            Intent(Intent.ACTION_VIEW).setDataAndType(uri, "application/pdf").addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
    }

    fun renameFile(row: io.github.dikshant363.pdfcraft.ui.home.FileRow, newName: String) {
        viewModelScope.launch {
            val name = DocumentFiles.safeName(newName.trim())
            val u = Uri.parse(row.key)
            val ok = withContext(Dispatchers.IO) {
                runCatching {
                    when {
                        u.scheme == "file" -> {
                            val f = File(requireNotNull(u.path))
                            val target = File(f.parentFile, name)
                            require(!target.exists()) { text(R.string.msg_rename_exists, name) }
                            require(f.renameTo(target)) { text(R.string.msg_rename_failed) }
                            recents.remove(setOf(row.key))
                            recents.touch(RecentFile(Uri.fromFile(target).toString(), name, System.currentTimeMillis(), 0, target.length()))
                            bookmarkStore.move(row.key, Uri.fromFile(target).toString())
                        }
                        // what the tools saved to Downloads: MediaStore entries keep their address when renamed
                        u.authority == android.provider.MediaStore.AUTHORITY -> {
                            val values = android.content.ContentValues().apply { put(android.provider.MediaStore.MediaColumns.DISPLAY_NAME, name) }
                            require(getApplication<Application>().contentResolver.update(u, values, null, null) > 0) { text(R.string.msg_rename_not_here) }
                            recents.update(row.key) { it.copy(name = name) }
                        }
                        else -> {
                            val renamed = android.provider.DocumentsContract.renameDocument(getApplication<Application>().contentResolver, u, name)
                                ?: error(text(R.string.msg_rename_not_here))
                            recents.remove(setOf(row.key))
                            recents.touch(RecentFile(renamed.toString(), name, System.currentTimeMillis(), 0, files.size(renamed) ?: 0))
                            bookmarkStore.move(row.key, renamed.toString())
                        }
                    }
                }
            }
            ok.onFailure { tell(it.userMessage) }
            refreshRecents()
            scanLocal(force = true)
        }
    }

    fun duplicateFile(row: io.github.dikshant363.pdfcraft.ui.home.FileRow) {
        viewModelScope.launch {
            val f = localFileOf(row) ?: return@launch tell(text(R.string.common_file_unreadable))
            val copy = withContext(Dispatchers.IO) { File(getApplication<Application>().cacheDir, "dup-${System.nanoTime()}.pdf").also { f.copyTo(it, overwrite = true) } }
            val base = row.name.removeSuffix(".pdf")
            val uri = withContext(Dispatchers.IO) { files.saveToDownloads(copy, "$base copy.pdf").also { copy.delete() } }
            tell(text(R.string.msg_duplicated, "$base copy.pdf"), text(R.string.common_open)) { open(uri) }
            scanLocal(force = true)
        }
    }

    private var copyRow: io.github.dikshant363.pdfcraft.ui.home.FileRow? = null

    fun saveFileCopy(row: io.github.dikshant363.pdfcraft.ui.home.FileRow) {
        copyRow = row
        saveCopyRequests.trySend(row.name)
    }

    /** The place chosen for a file row's Save a copy. */
    fun rowCopyTo(target: Uri?): Boolean {
        val row = copyRow ?: return false
        copyRow = null
        if (target == null) return true
        viewModelScope.launch {
            val f = localFileOf(row) ?: return@launch tell(text(R.string.common_file_unreadable))
            runCatching { withContext(Dispatchers.IO) { files.writeBack(f, target) } }
                .onSuccess { tell(text(R.string.msg_saved_copy)) }.onFailure { tell(it.userMessage) }
        }
        return true
    }

    fun deleteFile(row: io.github.dikshant363.pdfcraft.ui.home.FileRow) {
        viewModelScope.launch {
            val u = Uri.parse(row.key)
            val ok = withContext(Dispatchers.IO) {
                runCatching {
                    when {
                        u.scheme == "file" -> require(File(requireNotNull(u.path)).delete()) { text(R.string.msg_delete_failed) }
                        u.authority == android.provider.MediaStore.AUTHORITY ->
                            require(getApplication<Application>().contentResolver.delete(u, null, null) > 0) { text(R.string.msg_delete_failed) }
                        else -> require(android.provider.DocumentsContract.deleteDocument(getApplication<Application>().contentResolver, u)) { text(R.string.msg_delete_failed) }
                    }
                    recents.remove(setOf(row.key))
                }
            }
            ok.onFailure { tell(it.userMessage) }.onSuccess { tell(text(R.string.msg_file_deleted, row.name)) }
            refreshRecents()
            scanLocal(force = true)
        }
    }

    /** Photos: pick pictures and make them into a PDF (Create PDF from the photo picker). */
    val photoPicks = Channel<Unit>(Channel.CONFLATED)

    fun pickPhotosToPdf() {
        photoPicks.trySend(Unit)
    }

    fun onPhotosPicked(uris: List<Uri>) {
        if (uris.isNotEmpty()) onFilesPicked(FilePickKind.Create, uris)
    }

    // ---- Form auto-suggestions -----------------------------------------------------------------

    var formSuggestions by mutableStateOf(appPrefs.getBoolean("formSuggestions", true))
        private set

    fun updateFormSuggestions(on: Boolean) {
        formSuggestions = on
        appPrefs.edit().putBoolean("formSuggestions", on).apply()
    }

    /** Words typed into form fields before, newest first. */
    fun suggestionsFor(prefix: String): List<String> {
        if (!formSuggestions || prefix.isBlank()) return emptyList()
        val all = appPrefs.getString("suggestions", "").orEmpty().split('\n').filter { it.isNotBlank() }
        return all.filter { it.startsWith(prefix, ignoreCase = true) && !it.equals(prefix, ignoreCase = true) }.take(3)
    }

    fun rememberSuggestion(value: String) {
        if (!formSuggestions || value.isBlank() || value.length > 200) return
        val all = appPrefs.getString("suggestions", "").orEmpty().split('\n').filter { it.isNotBlank() && it != value }
        appPrefs.edit().putString("suggestions", (listOf(value) + all).take(200).joinToString("\n")).apply()
    }

    fun clearFormSuggestions() {
        appPrefs.edit().remove("suggestions").apply()
        tell(text(R.string.msg_suggestions_cleared))
    }

    // ---- Selecting a comment ------------------------------------------------------------------

    /** The comment tapped: its panel shows over the viewer. */
    var selectedComment by mutableStateOf<Pair<OpenDocument, io.github.dikshant363.pdfcraft.core.CommentInfo>?>(null)

    /** A tap at [at]: select the comment there (the top one), if any. */
    fun tapComment(doc: OpenDocument, at: PagePoint?): Boolean {
        if (at == null) return false
        val hit = doc.comments.lastOrNull { c ->
            c.page.toInt() == at.page && c.replyTo == null && c.state == null && c.rect.size == 4 &&
                at.x in (c.rect[0] - 4)..(c.rect[2] + 4) && at.y in (c.rect[1] - 4)..(c.rect[3] + 4)
        } ?: return false
        selectedComment = doc to hit
        return true
    }

    private fun commentArgs(c: io.github.dikshant363.pdfcraft.core.CommentInfo) = mapOf("page" to c.page.toInt() + 1, "index" to c.index.toInt() + 1)

    fun deleteComment(doc: OpenDocument, c: io.github.dikshant363.pdfcraft.core.CommentInfo) {
        selectedComment = null
        runTool(doc, "comment_delete", commentArgs(c))
    }

    fun recolorComment(doc: OpenDocument, c: io.github.dikshant363.pdfcraft.core.CommentInfo, color: androidx.compose.ui.graphics.Color) {
        runTool(doc, "comment_edit", commentArgs(c) + ("color" to color.hex())) { reselect(doc, c) }
    }

    /** Change a comment's opacity or line width (one undo step each). */
    fun styleComment(doc: OpenDocument, c: io.github.dikshant363.pdfcraft.core.CommentInfo, style: Map<String, Any?>) {
        runTool(doc, "comment_edit", commentArgs(c) + style) { reselect(doc, c) }
    }

    fun editCommentText(doc: OpenDocument, c: io.github.dikshant363.pdfcraft.core.CommentInfo, text: String) {
        runTool(doc, "comment_edit", commentArgs(c) + ("contents" to text)) { reselect(doc, c) }
    }

    fun moveComment(doc: OpenDocument, c: io.github.dikshant363.pdfcraft.core.CommentInfo, dx: Float, dy: Float) {
        runTool(doc, "comment_edit", commentArgs(c) + ("move" to listOf(dx.toDouble(), dy.toDouble()))) { reselect(doc, c) }
    }

    /** Put a comment in a new rectangle (points, top-left origin): a resize. */
    fun resizeComment(doc: OpenDocument, c: io.github.dikshant363.pdfcraft.core.CommentInfo, rect: List<Float>) {
        runTool(doc, "comment_edit", commentArgs(c) + ("rect" to rect.map { it.toDouble() })) { reselect(doc, c) }
    }

    /** Move or resize a Fill & Sign element, keeping it selected. */
    fun moveFill(doc: OpenDocument, c: io.github.dikshant363.pdfcraft.core.CommentInfo, dx: Float, dy: Float) {
        runTool(doc, "comment_edit", commentArgs(c) + ("move" to listOf(dx.toDouble(), dy.toDouble()))) {
            selectedFill = doc.comments.firstOrNull { it.page == c.page && it.index == c.index }
        }
    }

    fun resizeFill(doc: OpenDocument, c: io.github.dikshant363.pdfcraft.core.CommentInfo, rect: List<Float>) {
        runTool(doc, "comment_edit", commentArgs(c) + ("rect" to rect.map { it.toDouble() })) {
            selectedFill = doc.comments.firstOrNull { it.page == c.page && it.index == c.index }
        }
    }

    /** After an edit the comment is read again (same page and position). */
    private fun reselect(doc: OpenDocument, c: io.github.dikshant363.pdfcraft.core.CommentInfo) {
        val again = doc.comments.firstOrNull { it.page == c.page && it.index == c.index }
        selectedComment = again?.let { doc to it }
    }

    /** The Fill & Sign element tapped (its menu shows over it). */
    var selectedFill by mutableStateOf<io.github.dikshant363.pdfcraft.core.CommentInfo?>(null)

    /** Make a Fill & Sign element [factor] times bigger, about its top-left corner. */
    fun scaleFill(doc: OpenDocument, c: io.github.dikshant363.pdfcraft.core.CommentInfo, factor: Float) {
        if (c.rect.size < 4) return
        val w = (c.rect[2] - c.rect[0]) * factor
        val h = (c.rect[3] - c.rect[1]) * factor
        if (w < 3f || h < 3f || w > 2000f || h > 2000f) return
        val rect = listOf(c.rect[0], c.rect[1], c.rect[0] + w, c.rect[1] + h).map { it.toDouble() }
        runTool(doc, "comment_edit", commentArgs(c) + ("rect" to rect)) {
            selectedFill = doc.comments.firstOrNull { it.page == c.page && it.index == c.index }
        }
    }

    /**
     * Page [page]'s text reflowed into paragraphs from its text layer: lines are joined until one
     * ends a sentence well short of the line width, or a heading-like short line stands alone.
     */
    suspend fun reflowParagraphs(doc: OpenDocument, page: Int): List<io.github.dikshant363.pdfcraft.ui.viewer.Paragraph> {
        val eng = engine ?: return emptyList()
        val text = runCatching { eng.command { pagePlainText(doc.id, page.toUInt()) } }.getOrDefault("")
        val lines = text.lines().map { it.trim() }
        if (lines.none { it.isNotEmpty() }) return emptyList()
        val longest = lines.maxOf { it.length }.coerceAtLeast(1)
        val out = mutableListOf<io.github.dikshant363.pdfcraft.ui.viewer.Paragraph>()
        val current = StringBuilder()
        fun flush() {
            if (current.isNotBlank()) {
                out += io.github.dikshant363.pdfcraft.ui.viewer.Paragraph(page, out.size + 1, current.toString().trim(), floatArrayOf(0f, 0f, 0f, 0f), "", 12f, false, false, null)
            }
            current.clear()
        }
        for (line in lines) {
            if (line.isEmpty()) {
                flush()
                continue
            }
            // a hyphen at a line's end joins the word across it
            if (current.endsWith("-")) current.setLength(current.length - 1) else if (current.isNotEmpty()) current.append(' ')
            current.append(line)
            val short = line.length < longest * 0.6
            if (short && (line.last() in ".!?:" || line.length < longest * 0.35)) flush()
        }
        flush()
        return out
    }
}

/** The most files Create PDF converts at once. */
private const val MAX_CREATE_FILES = 12

/** A green banner saying a job worked, with the way to undo it. */
class SuccessBanner(val text: String, val undo: (() -> Unit)?)
